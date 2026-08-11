//! Yükleme STAGING alanı — `POST /uploads` (2026-08-11, K8 / Faz 3).
//!
//! Tasarım: `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`,
//! "K8" ve "`POST /uploads` — staging (Faz 3)" bölümleri.
//!
//! 500 MB'lık bir raporun baytlarının engine üzerinden (ya tek istekli `POST /wfe`
//! multipart yolu ya da rezervasyon yolu ile) geçmesi yanlış: bant genişliği, timeout
//! ve retry maliyeti engine'e biner. Bu modül baytları İSTEKTEN ÖNCE bir staging
//! alanına koyan yolu uygular: `routes::uploads::create_upload` bir `upload_id` üretir
//! (S3'te presigned PUT ile ya da local'de `PUT /uploads/{id}` ile doldurulur),
//! başlatma isteği yalnız bu tutamağı taşır; `take()` dosyayı doğrulayıp nihai anahtara
//! SERVER-SIDE COPY eder — dosya istemciye hiç geri inmez.
//!
//! ## Anahtar ailesi
//!
//! `staging/{upload_id}` — `attachments/{wfe_id}/{grup}/{item}` ve
//! `notes/{wfe_id}/{file_id}` köklerinden KASITLI olarak AYRI. Neden: bkz.
//! `crate::attachments::AttachmentStore::remove_all` yalnız o iki kökü tarar (WFE/
//! rezervasyon süpürmesinde "bu id'nin TÜM dosyaları" sorusunu oradan cevaplar).
//! Staging bu ağaca karışsaydı, henüz hiçbir WFE'ye bağlanmamış — belki de hiç
//! `take()` edilmeyecek — bir dosya "yüklenmiş" sayılırdı ve `status_for_node`/gate
//! mantığı yanlış cevap verirdi.
//!
//! ## Depo çözümü
//!
//! Staging nesnesi NİHAİ anahtarla **AYNI** depoda durmalıdır ki `take()` gerçek bir
//! server-side `Operator::copy` yapabilsin — ayrı depoda olsaydı taşıma "indir, tekrar
//! yükle" olurdu ve staging'in tüm amacı (baytları bir kez taşımak) boşa çıkardı. Depo
//! WFD BAŞINA `$env` ile çözülür (`crate::attachment_store::store_for_wfd`, DEĞİŞMEDİ);
//! `POST /uploads` anında seçilen `environment_id` DB satırına yazılır ve `take()` AYNI
//! ortamla depoyu çözer — rezervasyondaki (`wf.wfe_reservation`) sözleşmenin aynısı.
//!
//! ## Sahiplik + tazelik + süpürme
//!
//! `owned_by` deseni `crate::reservation::owned_by` ile birebir aynı: aynı tenant + aynı
//! aktör (org unit + kullanıcı). TTL de rezervasyonla AYNI (24 saat) — kullanıcının
//! belgeleri toplayıp başlatana kadar geçen makul üst sınır. `sweep_expired` ÖNCE nesneyi
//! sonra satırı siler (`reservation::release`'deki sıra gerekçesinin aynısı: ters sırada
//! satır gidince nesne artık hiçbir deftere bağlı kalmaz, sonsuza dek sahipsiz durur).
//!
//! ## `AttachmentStore`'a ihtiyaç duyulan tek satırlık ekleme
//!
//! Bu modül staging I/O'sunu `AttachmentStore`'un SARDIĞI `opendal::Operator` ile yapar
//! (`store_for_wfd`in döndürdüğü depo NİHAİ anahtarla aynı depo olduğu için — yukarıdaki
//! gerekçe). `AttachmentStore.op` alanı PRIVATE ve `crates/server/src/attachments.rs`
//! bu görevin dokunamadığı dosyalardan biri; bu yüzden aşağıdaki çağrılar
//! `AttachmentStore::operator(&self) -> &opendal::Operator` adında bir erişimci METOT
//! VARMIŞ gibi yazılmıştır. Bu metot `attachments.rs`'e EKLENMELİDİR (bkz. görev raporu).
//! Ayrı bir Operator kurmak (`wf_wfd::build_operator` burada tekrar çağırmak) BİLEREK
//! yapılmadı: `$env` çözümü ve Operator önbelleği TEK yerde (`attachment_store.rs`)
//! kalmalı, ikinci bir kurulum yolu aynı config için farklı bir Operator örneği (ve
//! önbellek ıskalaması) üretir.

use crate::attachments::Sha256Stream;
use crate::error::AppError;
use crate::state::AppState;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::types::actor::Actor;

/// Süresi dolmuş sayılan staging kaydının yaşı — `wf.wfe_reservation` ile AYNI süre
/// (bkz. `crate::reservation::TTL_HOURS`): kullanıcının belgeleri toplayıp başlatana
/// kadar geçen makul üst sınır. Ayrı bir sabite ihtiyaç yoktu ama iki modül birbirine
/// bağımlı kalmasın diye (biri değişirse diğeri sessizce kaymasın) burada kendi
/// kopyasını taşır — TTL'nin kavramsal olarak aynı olması tesadüf değil, ikisi de
/// "başlatılmamış girişimin makul ömrü" sorusuna cevap veriyor.
pub const TTL_HOURS: i64 = 24;

/// Bütünlük özeti çıkarılırken (server-side copy sonrası, bkz. `take`) bir seferde
/// okunan parça boyutu. TÜM dosyayı belleğe ALMAMAK için sabit boyutlu pencerelerle
/// okunur — 500 MB'lık bir dosyanın tamamı burada birikseydi K8'in bant genişliği/
/// bellek gerekçesi bu adımda boşa çıkardı.
const HASH_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// `wf.upload_staging` satırının Rust karşılığı.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Staged {
    pub upload_id: Uuid,
    pub orgtnt_id: Uuid,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub environment_id: Option<Uuid>,
    pub grp: String,
    pub item: String,
    pub actor_orgu_id: Uuid,
    pub actor_user_id: Uuid,
    pub created_at: DateTime<Utc>,
}

/// Staging nesnesinin storage anahtarı — bkz. modül başlığı "Anahtar ailesi".
pub fn staging_key(upload_id: Uuid) -> String {
    format!("staging/{upload_id}")
}

pub async fn create(pool: &PgPool, s: &Staged) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO wf.upload_staging \
           (upload_id, orgtnt_id, wfd_id, wfd_version, environment_id, grp, item, \
            actor_orgu_id, actor_user_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(s.upload_id)
    .bind(s.orgtnt_id)
    .bind(s.wfd_id)
    .bind(s.wfd_version)
    .bind(s.environment_id)
    .bind(&s.grp)
    .bind(&s.item)
    .bind(s.actor_orgu_id)
    .bind(s.actor_user_id)
    .execute(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

pub async fn get(pool: &PgPool, upload_id: Uuid) -> Result<Option<Staged>, AppError> {
    sqlx::query_as::<_, Staged>(
        "SELECT upload_id, orgtnt_id, wfd_id, wfd_version, environment_id, grp, item, \
                actor_orgu_id, actor_user_id, created_at \
           FROM wf.upload_staging WHERE upload_id = $1",
    )
    .bind(upload_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

/// Staging kaydı aynı tenant'ın ve aynı aktörün mü? Başkasının staging'ine dosya
/// yazmayı (`PUT /uploads/{id}`) ya da onu bir WFE'ye taşımayı (`take`) engeller.
/// `crate::reservation::owned_by` ile BİREBİR aynı desen.
pub fn owned_by(s: &Staged, orgtnt_id: Uuid, actor: &Actor) -> bool {
    s.orgtnt_id == orgtnt_id && s.actor_orgu_id == actor.orgu_id && s.actor_user_id == actor.user_id
}

pub async fn delete(pool: &PgPool, upload_id: Uuid) -> Result<(), AppError> {
    sqlx::query("DELETE FROM wf.upload_staging WHERE upload_id = $1")
        .bind(upload_id)
        .execute(pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(())
}

/// Süresi geçmiş sayılan staging kayıtları — `sweep_expired` bu listeyi kullanır.
async fn expired_rows(pool: &PgPool) -> Result<Vec<Staged>, AppError> {
    sqlx::query_as::<_, Staged>(
        "SELECT upload_id, orgtnt_id, wfd_id, wfd_version, environment_id, grp, item, \
                actor_orgu_id, actor_user_id, created_at \
           FROM wf.upload_staging \
          WHERE created_at < now() - ($1 || ' hours')::interval",
    )
    .bind(TTL_HOURS.to_string())
    .fetch_all(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

fn is_expired(s: &Staged) -> bool {
    Utc::now().signed_duration_since(s.created_at) > chrono::Duration::hours(TTL_HOURS)
}

/// `take()`in taşıdığı dosyanın sonucu — çağıran (`POST /wfe`nin
/// `payload.attachments[].upload_id` işleyicisi) bunu `wf.wfe_attachment` satırına
/// çevirir (bkz. `crate::wfe_attachment::AttachmentRow` — grup/item/boyut/tip/sha256
/// alanları birebir eşlenir).
pub struct TakenFile {
    pub grp: String,
    pub item: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
    pub sha256: String,
}

/// Staging'deki dosyayı nihai konumuna TAŞIR.
///
/// Sıra: (1) sahiplik + tazelik kontrolü — başkasının ya da süresi geçmiş bir staging
/// kaydı taşınamaz; (2) depoyu `store_for_wfd` ile çöz (staging KAYDEDİLDİĞİ ortamla,
/// NİHAİ anahtarla aynı depo — bkz. modül başlığı); (3) nesnenin var olduğunu ve
/// boyutunu `stat` ile doğrula (istemci `POST /uploads` aldı ama hiç PUT etmediyse ya
/// da presigned URL'e hiç yazmadıysa nesne yoktur); (4) `Operator::copy` ile
/// server-side taşı — backend desteklemiyorsa oku/yaz'a düş (bugünkü iki backend'de,
/// local `Fs` ve `S3`, `copy` DESTEKLENİYOR; bu dal yalnız savunma amaçlıdır, ör.
/// ileride eklenecek copy'siz bir servis için); (5) staging nesnesini ve satırı sil.
///
/// `dest_wfe_id`in kendisi DOĞRULANMAZ (var olduğu, aktörün görebildiği vs.) — bu
/// çağıranın (start commit akışı) sorumluluğudur; `take` yalnız "bu staging kaydı bu
/// hedefe taşınabilir mi" sorusuna cevap verir.
pub async fn take(
    state: &AppState,
    upload_id: Uuid,
    actor: &Actor,
    dest_wfe_id: Uuid,
) -> Result<TakenFile, AppError> {
    let staged = get(&state.pool, upload_id).await?.ok_or_else(|| AppError {
        message: format!("upload bulunamadı: {upload_id}"),
        status: StatusCode::NOT_FOUND,
        code: Some("upload_not_found"),
        items: None,
    })?;

    let orgtnt_id = state
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;

    if !owned_by(&staged, orgtnt_id, actor) {
        return Err(AppError(
            "bu yükleme size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if is_expired(&staged) {
        // Süresi geçmiş staging kaydı — süpürücü zaten dosyasını sahipsiz bulup
        // silecekti; burada erken ve NET bir hata dönmek "sessizce eski/olmayan bir
        // dosyayı taşı" ihtimalinden daha iyi. `upload_not_found` ile AYNI kod: istemci
        // için ayrım yok, ikisi de "bu handle artık geçerli değil, yeniden yükle" demek.
        return Err(AppError {
            message: "yükleme süresi doldu, yeniden yüklenmeli".into(),
            status: StatusCode::GONE,
            code: Some("upload_not_found"),
            items: None,
        });
    }

    // Taşıma bir YAZMADIR: hedef `attachments/{wfe_id}/…`. Depo `$env`de tanımlı
    // değilse sunucu diskine kopyalamak yerine 422 — bkz. routes/attachments.rs::resolve_target.
    let store = crate::attachment_store::store_for_wfd_strict(
        state,
        staged.wfd_id,
        orgtnt_id,
        staged.environment_id,
    )
    .await?;
    let op = store.operator();

    let src_key = staging_key(upload_id);
    let meta = op.stat(&src_key).await.map_err(|e| {
        if e.kind() == opendal::ErrorKind::NotFound {
            AppError {
                message: "yüklenen dosya bulunamadı (staging nesnesi yok — PUT hiç yapılmamış olabilir)"
                    .into(),
                status: StatusCode::NOT_FOUND,
                code: Some("upload_not_found"),
                items: None,
            }
        } else {
            AppError(
                format!("staging nesnesi okunamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        }
    })?;
    let size_bytes = meta.content_length();
    let content_type = meta.content_type().map(|s| s.to_string());
    let dest_key = format!("attachments/{dest_wfe_id}/{}/{}", staged.grp, staged.item);

    // Taşıma çekirdeği `move_to_final`e çıkarıldı — Faz 4'ün `promote`u AYNI adımı
    // (server-side copy, yoksa oku/yaz) ihtiyaç duyar, iki kopya istemedik.
    let sha256 = match move_to_final(op, &src_key, &dest_key).await? {
        // Oku/yaz'a düşüldü: özet zaten o geçişte çıkarıldı, ikinci okuma GEREKMEZ.
        Some(hash) => hash,
        // Server-side copy'de bayt hiçbir zaman APP belleğinden geçmedi — bütünlük
        // özeti ayrı bir stream-read geçişiyle, SABİT BOYUTLU pencerelerle çıkarılır
        // (bkz. `hash_object`), TÜM dosya tek seferde belleğe alınmadan.
        None => hash_object(op, &dest_key, size_bytes).await?,
    };

    // Nesne ÖNCE, satır SONRA — rezervasyondaki (`reservation::release`) sırayla AYNI
    // gerekçe TERS yönde uygulanır: burada dosya artık NİHAİ anahtarda güvende olduğu
    // için staging kopyasının hangi sırada silindiği önemsizdir (sahipsiz kalacak bir
    // şey yok), ama nesne silme satırdan ÖNCE denenir ki nesne silinemezse (örn. geçici
    // depo hatası) satır DB'de kalsın ve süpürücü bir dahaki turda tekrar dener —
    // tersi olsaydı (satır önce) ve nesne silinemezse, artık deftersiz kalan bir
    // staging nesnesi sonsuza dek diskte kalırdı.
    op.delete(&src_key).await.map_err(|e| {
        AppError(
            format!("staging nesnesi silinemedi: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    delete(&state.pool, upload_id).await?;

    Ok(TakenFile {
        grp: staged.grp,
        item: staged.item,
        size_bytes,
        content_type,
        sha256,
    })
}

/// `take`/`promote`in PAYLAŞTIĞI taşıma çekirdeği: staging nesnesini nihai anahtara
/// server-side `copy` ile taşır; backend desteklemiyorsa oku/yaz'a düşer (bugünkü iki
/// backend, local `Fs` ve `S3`, `copy`yi destekler — bu dal fiilen ölü koddur, yalnız
/// ileride eklenecek copy'siz bir servise karşı savunma; bkz. `take`in eski doc yorumu).
///
/// Dönüş özetin NEREDEN geleceğini bildirir: oku/yaz'a düşüldüyse bayt zaten sunucu
/// belleğinden geçti, o geçişte çıkarılan özet `Some` içinde döner — çağıran dosyayı
/// özet için İKİNCİ KEZ okumaz. Server-side copy'de bayt hiç sunucudan geçmedi, `None`
/// döner — özet gerekiyorsa çağıran ayrıca `hash_object` çağırmalı (`take` bunu yapar;
/// `promote` yapmaz, çünkü özet zaten `stage_part` sırasında stream'den çıkarılmıştı).
async fn move_to_final(
    op: &opendal::Operator,
    src_key: &str,
    dest_key: &str,
) -> Result<Option<String>, AppError> {
    if op.info().full_capability().copy {
        op.copy(src_key, dest_key).await.map_err(|e| {
            AppError(
                format!("dosya nihai konumuna taşınamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        Ok(None)
    } else {
        let bytes = op.read(src_key).await.map_err(|e| {
            AppError(
                format!("staging dosyası okunamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        let mut hasher = Sha256Stream::new();
        hasher.update(&bytes.to_bytes());
        op.write(dest_key, bytes).await.map_err(|e| {
            AppError(
                format!("dosya nihai konumuna yazılamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        Ok(Some(hasher.finish()))
    }
}

/// Bir nesnenin SHA-256 özetini, TÜM içeriği tek seferde belleğe almadan çıkarır —
/// `size_bytes` (önceden `stat` ile bilinen kesin uzunluk) sabit boyutlu pencerelere
/// bölünür, her pencere `Sha256Stream::update`e beslenir.
async fn hash_object(
    op: &opendal::Operator,
    key: &str,
    size_bytes: u64,
) -> Result<String, AppError> {
    let reader = op.reader(key).await.map_err(|e| {
        AppError(
            format!("bütünlük özeti için dosya açılamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    let mut hasher = Sha256Stream::new();
    let mut offset = 0u64;
    while offset < size_bytes {
        let end = std::cmp::min(offset + HASH_CHUNK_BYTES, size_bytes);
        let buf = reader.read(offset..end).await.map_err(|e| {
            AppError(
                format!("bütünlük özeti hesaplanamadı: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
        hasher.update(&buf.to_bytes());
        offset = end;
    }
    Ok(hasher.finish())
}

/// Süresi geçmiş staging kayıtlarını DOSYALARIYLA birlikte temizler. Mevcut saatlik
/// süpürücüye (`crate::reservation::sweep`) EKLENECEK — bu fonksiyon yalnız TEK bir
/// turu yapar, çağıran (bağlanacağı yer) döngüyü/zamanlamayı yönetir.
///
/// Bir satırın deposu çözülemezse (örn. `$env` sonradan eksildi, WFD silindi) o kayıt
/// ATLANIR ve süpürücü DURMAZ — `notes::sweep_expired_drafts`teki aynı savunma deseni.
pub async fn sweep_expired(state: &AppState) -> Result<u64, AppError> {
    let rows = expired_rows(&state.pool).await?;
    let mut swept = 0u64;
    for row in rows {
        let store = match crate::attachment_store::store_for_wfd(
            state,
            row.wfd_id,
            row.orgtnt_id,
            row.environment_id,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    upload_id = %row.upload_id,
                    "staging deposu çözülemedi, kayıt atlanıyor: {}", e.message
                );
                continue;
            }
        };
        let op = store.operator();
        // Önce nesne, sonra satır (bkz. `take` içindeki aynı sıra gerekçesi, burada da
        // geçerli: nesne silinip satır kalırsa bir daha yeniden denenir; tersi olsaydı
        // sahipsiz bir nesne sonsuza dek diskte kalırdı). `delete` opendal semantiğinde
        // idempotent — nesne zaten yoksa da hata vermez.
        if let Err(e) = op.delete(&staging_key(row.upload_id)).await {
            tracing::warn!(upload_id = %row.upload_id, "staging nesnesi silinemedi: {e}");
            continue;
        }
        if let Err(e) = delete(&state.pool, row.upload_id).await {
            tracing::warn!(
                upload_id = %row.upload_id,
                "staging satırı silinemedi: {}", e.message
            );
            continue;
        }
        swept += 1;
    }
    Ok(swept)
}

// ================= Faz 4: akış ortası (in-flight) çok-dosyalı yükleme =================
//
// `POST /wfe/{id}/actions` artık `multipart/form-data` da kabul edecek — WFE zaten var
// (Faz 3'ün aksine, henüz başlamamış bir başlatma değil, sürmekte olan bir akış). Burada
// değişmez şart farklı: **aksiyon uygulanamazsa mevcut belge DEĞİŞMEMELİ.** Nihai anahtar
// `attachments/{wfe_id}/{grup}/{item}` TEKTİR (sürümlenmiyor, `wf.wfe_attachment` yalnız
// SATIRI sürümler) — üstüne doğrudan yazıp aksiyon patlarsa eski baytlar GERİ GETİRİLEMEZ.
//
// Bu yüzden akış aynı staging alanından geçer: dosya ÖNCE `staging/{upload_id}`e yazılır
// (bu modülün Faz 3'te kurduğu AYNI anahtar ailesi, AYNI depo — `store_for_wfd_strict`);
// kapı kontrolü mevcut belge + staging'i BİRLİKTE görüp aksiyonu değerlendirir; aksiyon
// başarılıysa staging nihai anahtara COPY edilir (`promote`), başarısızsa staging silinir
// (`discard`) ve nihai anahtar hiç dokunulmamış kalır.

/// `stage_part`in ürettiği sonucun taşıdığı bilgi — çağıran (`POST /wfe/{id}/actions`
/// işleyicisi) bunu (a) aksiyon kapı kontrolüne (mevcut + staging birlikte), (b) aksiyon
/// BAŞARILIYSA `promote`a, (c) BAŞARISIZSA `discard`a verir.
pub struct StagedPart {
    pub upload_id: Uuid,
    pub grp: String,
    pub item: String,
    pub size_bytes: u64,
    pub content_type: Option<String>,
    pub sha256: String,
    /// İlk 64 bayt — çağıran magic-byte denetimi için ister (`attachments::detect_mismatch`,
    /// `routes/wfe.rs::start_multipart_committed`teki AYNI desen: tip kontrolü, dosya
    /// nihai anahtara gitmeden ÖNCE, ama baytlar zaten stream'den geçerken toplanır —
    /// dosyayı ikinci kez okumaya gerek kalmaz).
    pub head: Vec<u8>,
}

/// Akış ortası yüklemede tek bir dosyayı staging'e AKIŞ halinde yazar. `staged` çağıranın
/// `create` ile ÖNCEDEN açtığı satırdır (bu görevin WFE'si zaten var — `staged.wfd_id`/
/// `staged.environment_id`, o WFE'nin KENDİ depo çözümüyle AYNI olacak şekilde çağıran
/// tarafından doldurulmuş olmalı, yoksa `promote`teki `store_for_wfd_strict` FARKLI bir
/// depoya bakar ve server-side copy imkânsızlaşır — bkz. modül başlığı "Depo çözümü").
///
/// Dosya nihai anahtara HENÜZ gitmez: aksiyon başarılı olana kadar mevcut belge korunur.
/// `field`in gövdesi TÜMÜYLE belleğe ALINMAZ — `routes/uploads.rs::put_upload` ve
/// `routes/wfe.rs::start_multipart_committed`in kullandığı AYNI desen: chunk chunk
/// `Operator::writer`a yazılır, boyut sayacı ve `Sha256Stream` özeti aynı geçişte
/// hesaplanır (dosya ikinci kez okunmaz).
///
/// `field: &mut axum::extract::multipart::Field<'_>` seçildi (genel bir
/// `futures_util::Stream` ARABİRİMİ değil): çağıran zaten bir `axum::extract::Multipart`
/// üzerinde `next_field()` ile dolaşacak (`start_multipart_committed`teki desenin
/// birebir aynısı), `Field::chunk()` axum'un kendi hata tipini taşır ve `content_type()`/
/// `file_name()` gibi metotları zaten var — bunları ayrı bir `Stream`e sarmak gereksiz bir
/// dönüştürme katmanı eklerdi. `max_bytes` aşılırsa yazım `abort()` edilir (yarım nesne
/// TAMAMLANMAZ, S3'te multipart upload iptal edilir) ve `413` dönülür.
pub async fn stage_part(
    state: &AppState,
    staged: &Staged,
    field: &mut axum::extract::multipart::Field<'_>,
    max_bytes: usize,
) -> Result<StagedPart, AppError> {
    let store = crate::attachment_store::store_for_wfd_strict(
        state,
        staged.wfd_id,
        staged.orgtnt_id,
        staged.environment_id,
    )
    .await?;
    let op = store.operator();
    let key = staging_key(staged.upload_id);

    // İstemcinin bildirdiği Content-Type nesne metadata'sı olarak taşınır — `take()`in
    // `stat()`le okuduğu `content_type` boş kalmasın diye (`routes/uploads.rs::put_upload`
    // ile AYNI gerekçe).
    let declared_ct = field.content_type().map(str::to_string);
    let mut writer = match &declared_ct {
        Some(ct) => op.writer_with(&key).content_type(ct).await,
        None => op.writer(&key).await,
    }
    .map_err(|e| {
        AppError(
            format!("staging yazımı başlatılamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    let mut hasher = Sha256Stream::new();
    let mut head: Vec<u8> = Vec::with_capacity(64);
    let mut total: u64 = 0;

    while let Some(chunk) = field.chunk().await.map_err(|e| {
        AppError(format!("dosya akışı kesildi: {e}"), StatusCode::BAD_REQUEST)
    })? {
        total += chunk.len() as u64;
        if head.len() < 64 {
            let take = std::cmp::min(64 - head.len(), chunk.len());
            head.extend_from_slice(&chunk[..take]);
        }
        if total > max_bytes as u64 {
            // Yarım nesne TAMAMLANMASIN: mevcut belge zaten dokunulmamış duruyor, ama
            // staging'in kendisi de yarım kalmamalı — aksi halde `sweep_expired`e kadar
            // sahipsiz yarım bir nesne diskte durur (`routes/wfe.rs::start_multipart_
            // committed`teki `overflow` dalıyla AYNI desen).
            let _ = writer.abort().await;
            return Err(AppError(
                format!("dosya {max_bytes} bayt sınırını aşıyor"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        hasher.update(&chunk);
        writer.write(chunk).await.map_err(|e| {
            AppError(
                format!("staging yazımı başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    }
    writer.close().await.map_err(|e| {
        AppError(
            format!("staging yazımı kapatılamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    Ok(StagedPart {
        upload_id: staged.upload_id,
        grp: staged.grp.clone(),
        item: staged.item.clone(),
        size_bytes: total,
        content_type: declared_ct,
        sha256: hasher.finish(),
        head,
    })
}

/// Aksiyon BAŞARILI olduktan SONRA çağrılır: staging nesnesini nihai anahtara server-side
/// COPY ile taşır, staging nesnesini ve satırını siler. `take`teki taşıma çekirdeğiyle
/// (`move_to_final`) AYNI yardımcıyı kullanır — iki yerde iki kopya istemedik.
///
/// `part` depo çözümü için gereken (wfd_id, orgtnt_id, environment_id) üçlüsünü TAŞIMAZ
/// (yalnız `stage_part`in SONUCUNU taşır) — bu yüzden satır `get` ile YENİDEN okunur; satır
/// `stage_part`ın açtığı `create` çağrısından beri DB'de durur, `promote`/`discard`
/// çağrılana kadar silinmez. Satır bulunamazsa (örn. aynı parça iki kez `promote`
/// edilmeye çalışıldı) `upload_not_found` — çağıranın hata yolu bunu ele almalı.
///
/// Bütünlük özeti YENİDEN HESAPLANMAZ: `stage_part` baytları stream ederken zaten
/// çıkarmıştı (`part.sha256`), `move_to_final`in oku/yaz dalı bir özet üretse de burada
/// KASITLI OLARAK yok sayılır — aynı dosyayı ikinci kez taramak gereksiz iş olurdu.
pub async fn promote(state: &AppState, part: &StagedPart, dest_wfe_id: Uuid) -> Result<(), AppError> {
    let staged = get(&state.pool, part.upload_id).await?.ok_or_else(|| AppError {
        message: format!("staging kaydı bulunamadı: {}", part.upload_id),
        status: StatusCode::NOT_FOUND,
        code: Some("upload_not_found"),
        items: None,
    })?;

    let store = crate::attachment_store::store_for_wfd_strict(
        state,
        staged.wfd_id,
        staged.orgtnt_id,
        staged.environment_id,
    )
    .await?;
    let op = store.operator();

    let src_key = staging_key(part.upload_id);
    let dest_key = format!("attachments/{dest_wfe_id}/{}/{}", part.grp, part.item);
    move_to_final(op, &src_key, &dest_key).await?;

    // Nesne ÖNCE, satır SONRA — `take`teki AYNI sıra gerekçesi (nesne silinemezse satır
    // DB'de kalır, `sweep_expired` bir dahaki turda tekrar dener).
    op.delete(&src_key).await.map_err(|e| {
        AppError(
            format!("staging nesnesi silinemedi: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    delete(&state.pool, part.upload_id).await?;
    Ok(())
}

/// Aksiyon BAŞARISIZ olduktan SONRA çağrılır: bu istekte `stage_part` edilmiş TÜM
/// parçaların staging nesnesi + satırı silinir, nihai anahtara HİÇ DOKUNULMAZ (zaten
/// `promote` çağrılmadığı için nihai anahtar bu parçalar için hiçbir zaman var olmadı).
///
/// Hata YUTULUR ve `warn` loglanır — bu, aksiyonun BAŞARISIZLIK sebebini (asıl hata)
/// gölgelemesin diye kasıtlı: çağıran zaten bir hata dönüyor, temizlik ikinci bir hata
/// üretip onu MASKELEMEMELİ. Silinemeyen bir satır kalıcı sızıntı değildir — TTL'si
/// dolunca `sweep_expired` onu dosyasıyla birlikte toplar (bkz. modül başlığı).
pub async fn discard(state: &AppState, parts: &[StagedPart]) {
    for part in parts {
        let staged = match get(&state.pool, part.upload_id).await {
            Ok(Some(s)) => s,
            // Satır zaten yok (örn. bu parça aynı istekte daha önce silinmeye çalışıldı,
            // ya da hiç yazılmamıştı) — yapacak iş yok.
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(
                    upload_id = %part.upload_id,
                    "staging kaydı okunamadı, silme atlanıyor: {}", e.message
                );
                continue;
            }
        };
        let store = match crate::attachment_store::store_for_wfd_strict(
            state,
            staged.wfd_id,
            staged.orgtnt_id,
            staged.environment_id,
        )
        .await
        {
            Ok(s) => s,
            Err(e) => {
                // Depo çözülemedi (örn. `$env` bu arada değişti) — satır sahipsiz kalır,
                // `sweep_expired` TTL dolunca toplar (bkz. o fonksiyonun AYNI savunması).
                tracing::warn!(
                    upload_id = %part.upload_id,
                    "staging deposu çözülemedi, kayıt sweep_expired'a bırakılıyor: {}", e.message
                );
                continue;
            }
        };
        let op = store.operator();
        if let Err(e) = op.delete(&staging_key(part.upload_id)).await {
            tracing::warn!(upload_id = %part.upload_id, "staging nesnesi silinemedi: {e}");
            continue;
        }
        if let Err(e) = delete(&state.pool, part.upload_id).await {
            tracing::warn!(
                upload_id = %part.upload_id,
                "staging satırı silinemedi: {}", e.message
            );
        }
    }
}
