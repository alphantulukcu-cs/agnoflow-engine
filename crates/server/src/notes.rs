//! WFE not defteri — ortak mantık (Faz 1, SADECE METİN). `crate::attachments`'ın
//! kardeşidir: gerçek iş burada, iki route ağacı (`routes/notes.rs` X-Actor,
//! `routes/portal/notes.rs` JWT) ince kabuk olarak bunu çağırır.
//!
//! Tasarım: `docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`.
//! Engine core (`wfe-core`/`wfe`) bu katmandan habersizdir — not motorun ne
//! context'ine (`$ctx`) ne defterine (`$wfah`) yazılır (K1). Yetki modeli ayrı
//! icat edilmez: not, bağlı olduğu WFE'nin görünürlüğünü miras alır (K6) —
//! çağıran taraf her uçta önce `executor.query(wfe_id, actor)` ile bunu doğrular,
//! bu modül yalnız not-seviyesi (draft sahipliği, durum) kuralları uygular.
//!
//! Dosya ekleme (`wf.wfe_note_file`, Faz 2) BU MODÜLDEDİR — depo I/O'su değil,
//! yalnız metadata + limit doğrulaması; gerçek blob yazımı/okuması
//! `crate::attachments::AttachmentStore::note_write/note_read/note_delete`
//! (route katmanı, `attachment_store::store_for_wfe_strict` ile çözülür, K4).
//! `audience` süzgeci (K9) ve okundu takibi (`wf.wfe_note_read`, Faz 3) BU
//! MODÜLDEDİR: `list_visible`/`count_by_wfe` audience'a göre süzer,
//! `unread_count_by_wfe`/`mark_read` okundu izini yönetir.
//!
//! Alt akış (WFC) notları (K8, Faz 4-runtime): `list_visible_with_children`
//! çağıranın notlarına, `notes_visible_to_caller: true` bayraklı çağrılarının
//! ÇOCUK WFE'sinin published notlarını `from_call` etiketiyle katar. Bu
//! fonksiyon `wfe_core::types::wfd_v22::Wfd` okur (motora YAZMAZ) — yalnız
//! `CallRef.notes_visible_to_caller` bayrağını taşıdığı için (motor bu alanı
//! hiç okumaz), bu katmanın onu okuması engine core'u bu modüle bağımlı KILMAZ.

use crate::error::AppError;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::ports::BranchStatus;

/// Not hedefleme (K9). `{"kind":"all"}` (varsayılan) → herkes (WFE'yi
/// görebilen); `{"kind":"users","ids":[...]}` → yalnız listelenen
/// `user_id`'ler (+ notun yazarı, her koşulda). Başka bir `kind` deserialize
/// hatası üretir — axum'un `Json` extractor'ı bunu `400`'e çevirir, ayrıca bir
/// doğrulama kodu yazmaya gerek yoktur.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Audience {
    All,
    Users { ids: Vec<Uuid> },
}

impl Default for Audience {
    fn default() -> Self {
        Audience::All
    }
}

/// `audience` süzgeci ORTAK SQL parçası — hem `list_visible` hem
/// `count_by_wfe`/`unread_count_by_wfe` hem çocuk WFE not sorgusu aynı kuralı
/// uygular: `{"kind":"all"}` HERKESE görünür, `{"kind":"users"}` yalnız
/// listedeki `ids`'e VE notun yazarına. `?` jsonb operatörü dizi İÇİNDE metin
/// eşleşmesi arar (`audience->'ids'` bir metin dizisidir).
///
/// `prefix` çıplak kolon sorgularında `""`, JOIN'li (`unread_count_by_wfe`,
/// `mark_read`) sorgularda tablo alias'ı (`"n."`) — kolon adı belirsizliğini
/// önler. `user_param`/`orgu_param` gerçek bind yer tutucusudur (`"$2"` vb.).
fn audience_sql(prefix: &str, user_param: &str, orgu_param: &str) -> String {
    format!(
        "({prefix}audience->>'kind' = 'all' \
          OR {prefix}audience->'ids' ? {user_param}::text \
          OR ({prefix}author_user_id = {user_param} AND {prefix}author_orgu_id = {orgu_param}))"
    )
}

/// Not gövdesi uzunluk sınırı (karakter). Aşımı `400`.
pub const MAX_BODY_LEN: usize = 10_000;

/// Yetim draft TTL'i (saat) — süpürücü bundan eski taslakları siler (K5).
pub const DRAFT_TTL_HOURS: i64 = 24;

// ---- Ad-hoc dosya limitleri (Faz 2, spec "Ad-hoc dosya limitleri") ----
//
// Katalog `AttachmentItem.formats` kuralları burada uygulanamaz (ad-hoc dosyanın
// katalogda karşılığı yok) — sınırsız yükleme yüzeyi doğmasın diye sunucu tarafı
// sabitleri konur.

/// Dosya başı boyut sınırı (bayt) — aşımı `413 note.too_large`.
pub const MAX_FILE_BYTES: i64 = 25 * 1024 * 1024;

/// Not başına dosya sayısı sınırı — aşımı `422`.
pub const MAX_FILES_PER_NOTE: i64 = 10;

/// WFE başına TÜM notların dosyalarının toplam boyutu — aşımı `422`. Not bazlı
/// sayı sınırı tek başına yeterli değildir: az sayıda çok büyük dosya da aynı
/// sınırsız yükleme yüzeyini açar.
pub const MAX_WFE_QUOTA_BYTES: i64 = 200 * 1024 * 1024;

/// Dosya adı uzunluk sınırı (karakter) — `sanitize_filename` bu sınıra kısar.
pub const MAX_FILENAME_LEN: usize = 255;

/// Çalıştırılabilir/tehlikeli MIME blocklist — ad-hoc not dosyasında YASAK
/// (`415 note.unsupported_type`). Katalogun `formats` allowlist'inin aksine
/// burada allowlist yok (herhangi bir belge/görsel/ofis dosyası serbest);
/// yalnız bilinen çalıştırılabilir tipler reddedilir.
pub const MIME_BLOCKLIST: &[&str] = &[
    "application/x-msdownload",
    "application/x-executable",
    "application/x-sh",
    "application/x-bat",
    "application/x-msdos-program",
    "application/vnd.microsoft.portable-executable",
    "application/java-archive",
    "application/x-elf",
    "application/vnd.apple.installer+xml",
];

/// Dosya adını sanitize eder: yol ayracı (`/`, `\`), `..` ve kontrol karakterleri
/// temizlenir, uzunluk `MAX_FILENAME_LEN`'e kısılır. Boş/temizlik-sonrası-boş
/// isim `"dosya"`a düşer — `Content-Disposition` başlığı hiçbir zaman boş kalmaz.
/// `X-Filename` başlığındaki yüzde-kodlamayı ÇÖZER (UTF-8).
///
/// HTTP başlıkları ISO-8859-1 dışına çıkamaz; "Kredi Sözleşmesi.pdf" gibi bir ad
/// başlığa ancak kodlanarak konabilir. Çözümü İSTEMCİYE bırakmak sözleşmeyi
/// istemci geleneğine indirger: kodlayan istemcinin yazdığı adı, kodlamayan
/// istemci bozuk görür ve DB'de gerçek ad yerine `%C3%B6` yığını kalır. Bu yüzden
/// çözüm SUNUCUDA: DB'de daima gerçek ad durur, her istemci aynı adı görür.
/// Kodlanmamış ad da bozulmadan geçer (`%` üçlüsü değilse harf olarak kalır);
/// geçersiz UTF-8 çıkarsa ham metne düşülür.
pub fn decode_filename(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let decoded = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(b) = decoded {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_string())
}

pub fn sanitize_filename(name: &str) -> String {
    let no_control: String = name
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\')
        .collect();
    let no_traversal = no_control.replace("..", "");
    let trimmed = no_traversal.trim();
    let base = if trimmed.is_empty() { "dosya" } else { trimmed };
    base.chars().take(MAX_FILENAME_LEN).collect()
}

/// MIME blocklist kontrolü — `Content-Type`'ın parametre kısmı (`; charset=...`)
/// yok sayılır, karşılaştırma büyük/küçük harf duyarsızdır.
pub fn is_blocked_mime(mime: &str) -> bool {
    let m = mime.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    MIME_BLOCKLIST.iter().any(|b| *b == m)
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NoteRow {
    pub note_id: Uuid,
    pub wfe_id: Uuid,
    #[allow(dead_code)] // Sorgu SELECT * yerine kolon adıyla yazılıyor; tenant izolasyonu
    // bugün wfe_id/executor.query üzerinden sağlanır, bu alan okunmaz.
    pub orgtnt_id: Uuid,
    pub author_orgu_id: Uuid,
    pub author_user_id: Uuid,
    pub author_role: String,
    pub body: String,
    pub node: Option<String>,
    pub wfah_seq: Option<i32>,
    // K9: süzgeç SQL WHERE'de uygulanır (`audience_sql`) — bu alan Rust
    // tarafında hiç okunmaz, `FromRow` kolonu doldurmak için taşınır.
    #[allow(dead_code)]
    pub audience: serde_json::Value,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    #[allow(dead_code)] // Bugün API'ye çıkmıyor; ileri fazlarda denetim izinde kullanılabilir.
    pub hidden_by: Option<Uuid>,
}

const NOTE_COLUMNS: &str = "note_id, wfe_id, orgtnt_id, author_orgu_id, author_user_id, \
    author_role, body, node, wfah_seq, audience, status, created_at, published_at, \
    hidden_at, hidden_by";

/// Not dosyası satırı (`wf.wfe_note_file`, Faz 2). `note_id` scope/IDOR
/// çözümü içindir (bkz. `find_file`); `storage_key` API'ye çıkmaz — gerçek
/// anahtar `AttachmentStore::note_*` tarafından `wfe_id`+`file_id`'den
/// yeniden türetilir, bu kolon yalnız iz/denetim amaçlıdır.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct NoteFileRow {
    pub file_id: Uuid,
    pub note_id: Uuid,
    pub filename: String,
    pub mime: String,
    pub size_bytes: i64,
    #[allow(dead_code)]
    pub storage_key: String,
    pub created_at: DateTime<Utc>,
}

const NOTE_FILE_COLUMNS: &str =
    "file_id, note_id, filename, mime, size_bytes, storage_key, created_at";

/// Not dosyası — API görünümü (`storage_key` sızmaz).
#[derive(Debug, Serialize, Clone)]
pub struct NoteFileView {
    pub file_id: Uuid,
    pub filename: String,
    pub mime: String,
    pub size_bytes: i64,
    pub created_at: DateTime<Utc>,
}

impl From<NoteFileRow> for NoteFileView {
    fn from(r: NoteFileRow) -> Self {
        NoteFileView {
            file_id: r.file_id,
            filename: r.filename,
            mime: r.mime,
            size_bytes: r.size_bytes,
            created_at: r.created_at,
        }
    }
}

/// Not gövdesi — gizlenmişse (K3) `body`/`files` yerine `{hidden:true}` döner;
/// gövde ve dosya listesi DB'de kalır, API'den asla sızmaz. Dosyalar notun
/// İÇERİĞİNİN parçasıdır — gizlenen bir notun ekli belgesi de erişilemez kalır,
/// aksi halde "gizleme" gövdeyi kapatıp belgeyi açık bırakırdı.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum NoteContent {
    Visible { body: String, files: Vec<NoteFileView> },
    Hidden { hidden: bool },
}

#[derive(Debug, Serialize)]
pub struct NoteView {
    pub note_id: Uuid,
    pub wfe_id: Uuid,
    pub author_user_id: Uuid,
    pub author_role: String,
    pub status: String,
    pub node: Option<String>,
    pub wfah_seq: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    /// Faz 3 (K9 okundu takibi): bu AKTÖR bu notu okumuş mu. Kendi yazdığın
    /// not (draft ya da published) daima `true`'dur — yazma zaten okumadır.
    pub read: bool,
    /// Faz 4-runtime (K8): bu not ÇAĞIRANIN kendi defterinde değil, bir alt
    /// akıştan `notes_visible_to_caller: true` ile katılmışsa çağrının
    /// `call_key`'i; kendi notlarında `None` (alan hiç çıkmaz).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_call: Option<String>,
    #[serde(flatten)]
    pub content: NoteContent,
}

/// `NoteRow` + önceden TOPLU çekilmiş dosya listesini/okundu bilgisini
/// `NoteView`e birleştirir (bkz. `list_visible` — N+1 yok). Gizli notta
/// `files` hiç taşınmaz. `from_call` burada hep `None`; çağıran taraf
/// (`list_visible_with_children`) çocuk notlarını sonradan etiketler.
fn note_view(r: NoteRow, files: Vec<NoteFileView>, read: bool) -> NoteView {
    let content = if r.hidden_at.is_some() {
        NoteContent::Hidden { hidden: true }
    } else {
        NoteContent::Visible { body: r.body, files }
    };
    NoteView {
        note_id: r.note_id,
        wfe_id: r.wfe_id,
        author_user_id: r.author_user_id,
        author_role: r.author_role,
        status: r.status,
        node: r.node,
        wfah_seq: r.wfah_seq,
        created_at: r.created_at,
        published_at: r.published_at,
        read,
        from_call: None,
        content,
    }
}

fn db_err(e: sqlx::Error) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

fn check_body_len(body: &str) -> Result<(), AppError> {
    if body.chars().count() > MAX_BODY_LEN {
        return Err(AppError(
            format!("not gövdesi {MAX_BODY_LEN} karakteri aşamaz"),
            StatusCode::BAD_REQUEST,
        ));
    }
    Ok(())
}

/// Notu WFE KAPSAMINDA arar. `wfe_id` filtresi şart: yetki (K6) yol
/// parametresindeki WFE üzerinden veriliyor, mutasyon ise `note_id` ile
/// hedefleniyor. İkisi bağlanmazsa bir WFE'yi görebilen aktör, eline geçen
/// BAŞKA bir WFE'ye ait `note_id` ile o notu düzenleyebilir/gizleyebilirdi.
/// Kapsam dışı not "bulunamadı"dır — varlığı da sızmaz.
async fn find_note(pool: &PgPool, wfe_id: Uuid, note_id: Uuid) -> Result<NoteRow, AppError> {
    sqlx::query_as::<_, NoteRow>(&format!(
        "SELECT {NOTE_COLUMNS} FROM wf.wfe_note WHERE note_id = $1 AND wfe_id = $2"
    ))
    .bind(note_id)
    .bind(wfe_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or_else(|| AppError("not bulunamadı".into(), StatusCode::NOT_FOUND))
}

/// Draft'ın SAHİBİ mi? Rol dahil değil — aynı kişi rol değiştirse de kendi
/// taslağını görebilmeli (`reservation::owned_by` ile aynı yaklaşım: orgu+user).
fn is_author(row: &NoteRow, actor: &Actor) -> bool {
    row.author_orgu_id == actor.orgu_id && row.author_user_id == actor.user_id
}

/// Dosya OKUMA (indirme) kapısı — `is_author` mutasyon kapısının salt-okunur
/// eşdeğeri. Draft: yalnız yazarı (403). Published+gizlenmiş: K3 gereği içerik
/// (gövde ve onun parçası olan dosyalar) API'den sızmaz → `404` (var olduğu da
/// sızmayan `find_note` ile aynı gerekçe; gizlemenin gövdeyi kapatıp dosyayı
/// açık bırakması "değiştirilemez" sözleşmesini delerdi).
fn assert_file_readable(row: &NoteRow, actor: &Actor) -> Result<(), AppError> {
    if row.status == "draft" {
        if !is_author(row, actor) {
            return Err(AppError(
                "bu not size ait değil".into(),
                StatusCode::FORBIDDEN,
            ));
        }
    } else if row.hidden_at.is_some() {
        return Err(AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    Ok(())
}

/// Draft not yaratır — yalnız yazarı görür, `note_id` döner. Yetki (K6, WFE
/// görünürlüğü) ÇAĞIRAN TARAFTA doğrulanmış olmalı. `audience` (K9) draft
/// aşamasında da yazılır — draft'ı zaten yalnız yazarı görüyor, ama publish'te
/// hedeflemenin sonradan eklenmesi yerine baştan belirlenmesi daha basittir.
pub async fn create_draft(
    pool: &PgPool,
    wfe_id: Uuid,
    orgtnt_id: Uuid,
    actor: &Actor,
    body: String,
    audience: Audience,
) -> Result<Uuid, AppError> {
    check_body_len(&body)?;
    let audience_json = serde_json::to_value(&audience)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let note_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO wf.wfe_note \
           (note_id, wfe_id, orgtnt_id, author_orgu_id, author_user_id, author_role, body, \
            audience, status) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'draft')",
    )
    .bind(note_id)
    .bind(wfe_id)
    .bind(orgtnt_id)
    .bind(actor.orgu_id)
    .bind(actor.user_id)
    .bind(&actor.role)
    .bind(body)
    .bind(audience_json)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(note_id)
}

/// Draft gövdesini düzenler — yalnız yazarı (403), yalnız `status='draft'`
/// (aksi halde 409 `note.immutable`, K3).
pub async fn update_draft(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
    body: String,
) -> Result<(), AppError> {
    check_body_len(&body)?;
    let row = find_note(pool, wfe_id, note_id).await?;
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.status != "draft" {
        return Err(AppError {
            message: "yayınlanmış not düzenlenemez".into(),
            status: StatusCode::CONFLICT,
            code: Some("note.immutable"),
            items: None,
        });
    }
    sqlx::query("UPDATE wf.wfe_note SET body = $1 WHERE note_id = $2")
        .bind(body)
        .bind(note_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// Draft'ı yayınlar — motorun defterine (`wf.wfah`) çapa atar (K5/K7). `status`
/// `draft` değilse 409 `note.not_draft` (zaten yayınlanmış notu tekrar yayınlama
/// girişimi).
///
/// **`pub` DEĞİL** (2026-08-11): serbest yayın (`wfah_seq`/`node` = `None`,
/// aksiyona bağlı olmayan not) KALDIRILDI — dışa açık tek yayın yolu
/// `publish_after_apply`/`republish_after_apply`, ikisi de çapa geçer. İmza hâlâ
/// `Option` alıyor çünkü apply'ın `from_node`'u NULL olabilir (fork/join sistem
/// geçişleri); `wfah_seq: None` çağrısı artık bu modülde bile yapılmaz.
async fn publish(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
    wfah_seq: Option<i32>,
    node: Option<&str>,
) -> Result<(), AppError> {
    let row = find_note(pool, wfe_id, note_id).await?;
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.status != "draft" {
        return Err(AppError {
            message: "not zaten yayınlanmış".into(),
            status: StatusCode::CONFLICT,
            code: Some("note.not_draft"),
            items: None,
        });
    }
    sqlx::query(
        "UPDATE wf.wfe_note \
            SET status = 'published', published_at = now(), wfah_seq = $1, node = $2 \
          WHERE note_id = $3",
    )
    .bind(wfah_seq)
    .bind(node)
    .bind(note_id)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Draft ise satırı SİLER (yalnız yazarı — 403 başkasına); published ise
/// `hidden_at`/`hidden_by` doldurur, `body` ASLA UPDATE edilmez (K3).
pub async fn hide(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
) -> Result<(), AppError> {
    let row = find_note(pool, wfe_id, note_id).await?;
    // Gizleme de yazarına aittir. WFE'yi görebilen HERKES gizleyebilseydi, bir not
    // (karar delili, K3) hedefi tarafından ekrandan kaldırılabilirdi — "değiştirilemez"
    // sözleşmesi gizleme kanalından delinirdi.
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.status == "draft" {
        sqlx::query("DELETE FROM wf.wfe_note WHERE note_id = $1")
            .bind(note_id)
            .execute(pool)
            .await
            .map_err(db_err)?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE wf.wfe_note SET hidden_at = now(), hidden_by = $1 \
          WHERE note_id = $2 AND hidden_at IS NULL",
    )
    .bind(actor.user_id)
    .bind(note_id)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Gizlemeyi GERİ ALIR (`hidden_at`/`hidden_by` → NULL) — gövde ve dosyalar
/// yeniden görünür olur. Yalnız YAZARI, yani gizleyebilen kişinin ta kendisi
/// (`hide` ile aynı kapı): gizleme geri alınamaz olsaydı yanlışlıkla basılan bir
/// düğme notu kalıcı olarak ekrandan silerdi — oysa K3'ün koruduğu şey gövdenin
/// DEĞİŞMEZLİĞİDİR, görünürlüğün tek yönlülüğü değil. Gövde zaten hiç UPDATE
/// edilmiyor; gizle/göster yalnız bir bayrağı çevirir ve `hidden_by` her seferinde
/// kimin çevirdiğini yazar.
///
/// Draft'ta anlamsızdır (draft gizlenmez, SİLİNİR) → 409 `note.not_hidden`;
/// zaten görünür bir notta da aynı kod döner (yutulan no-op yerine açık cevap).
pub async fn unhide(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
) -> Result<(), AppError> {
    let row = find_note(pool, wfe_id, note_id).await?;
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.hidden_at.is_none() {
        return Err(AppError {
            message: "not zaten görünür".into(),
            status: StatusCode::CONFLICT,
            code: Some("note.not_hidden"),
            items: None,
        });
    }
    sqlx::query(
        "UPDATE wf.wfe_note SET hidden_at = NULL, hidden_by = NULL \
          WHERE note_id = $1 AND hidden_at IS NOT NULL",
    )
    .bind(note_id)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Görünür notlar: (`status='published'` VE `audience` aktöre/yazara açık) +
/// aktörün KENDİ draft'ları, `created_at ASC`. `audience` süzgeci (K9)
/// draft'a UYGULANMAZ — draft kuralı değişmez, hâlâ "yalnız yazarı" (K6).
pub async fn list_visible(
    pool: &PgPool,
    wfe_id: Uuid,
    actor: &Actor,
) -> Result<Vec<NoteView>, AppError> {
    let audience = audience_sql("", "$2", "$3");
    let rows = sqlx::query_as::<_, NoteRow>(&format!(
        "SELECT {NOTE_COLUMNS} FROM wf.wfe_note \
          WHERE wfe_id = $1 \
            AND ((status = 'published' AND {audience}) \
                 OR (status = 'draft' AND author_user_id = $2 AND author_orgu_id = $3)) \
          ORDER BY created_at ASC"
    ))
    .bind(wfe_id)
    .bind(actor.user_id)
    .bind(actor.orgu_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows_to_views(pool, rows, actor).await
}

/// Yalnız `status='published'` olan görünür notlar — draft'lar HİÇBİR koşulda
/// (aktör yazar dahi olsa) dönmez. Faz 4-runtime (K8): çocuk WFE'nin notları
/// çağırana katılırken kullanılan yol budur — "draft'lar sızmaz" kuralı burada
/// `list_visible`'ın draft OR kolunu hiç yazmayarak, koşulla değil, sorgunun
/// KENDİSİYLE garanti edilir.
async fn list_visible_published_only(
    pool: &PgPool,
    wfe_id: Uuid,
    actor: &Actor,
) -> Result<Vec<NoteView>, AppError> {
    let audience = audience_sql("", "$2", "$3");
    let rows = sqlx::query_as::<_, NoteRow>(&format!(
        "SELECT {NOTE_COLUMNS} FROM wf.wfe_note \
          WHERE wfe_id = $1 AND status = 'published' AND {audience} \
          ORDER BY created_at ASC"
    ))
    .bind(wfe_id)
    .bind(actor.user_id)
    .bind(actor.orgu_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    rows_to_views(pool, rows, actor).await
}

/// `list_visible`/`list_visible_published_only` ORTAK kuyruğu: dosya listesini
/// ve okundu bilgisini TOPLU (N+1 yok) çekip `NoteView`e birleştirir.
async fn rows_to_views(
    pool: &PgPool,
    rows: Vec<NoteRow>,
    actor: &Actor,
) -> Result<Vec<NoteView>, AppError> {
    let note_ids: Vec<Uuid> = rows.iter().map(|r| r.note_id).collect();
    let mut files_by_note = files_for_notes(pool, &note_ids).await?;
    let read_notes = reads_for_notes(pool, &note_ids, actor.user_id).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let files = files_by_note.remove(&r.note_id).unwrap_or_default();
            // Kendi yazdığın not daima okunmuş sayılır (K9 okundu takibi) —
            // yazma zaten okumadır, ayrıca `wfe_note_read` satırı gerekmez.
            let read = is_author(&r, actor) || read_notes.contains(&r.note_id);
            note_view(r, files, read)
        })
        .collect())
}

/// Verilen notlardan bu AKTÖRÜN okuduklarının kümesi — TEK sorgu, N+1 yok.
async fn reads_for_notes(
    pool: &PgPool,
    note_ids: &[Uuid],
    user_id: Uuid,
) -> Result<HashSet<Uuid>, AppError> {
    if note_ids.is_empty() {
        return Ok(HashSet::new());
    }
    let rows: Vec<Uuid> = sqlx::query_scalar(
        "SELECT note_id FROM wf.wfe_note_read WHERE note_id = ANY($1) AND user_id = $2",
    )
    .bind(note_ids)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

/// Faz 4-runtime (K8): çağıranın görünür notlarına, bu WFE'nin YAPTIĞI
/// çağrılardan `notes_visible_to_caller: true` olanların ÇOCUK WFE'sinin
/// published notlarını `from_call` (çağrı key'i) etiketiyle katar.
///
/// Yetki: aktör bu (çağıran) WFE'yi görebiliyorsa YETER — bayrağın kendisi
/// yetkidir, çocuk WFE için ayrı `executor.query` KOŞULMAZ. Bu, çocuğu
/// göremeyen ama ebeveyni gören bir kullanıcıya WFD tasarımcısının BİLİNÇLİ
/// açtığı bir kapıdır (K8): `notes_visible_to_caller` node/terminal
/// yerleşiminde false varsayılanla kapalıdır, tasarımcı açıkça açar.
///
/// Yalnız BİR seviye derine inilir — çocuğun kendi çağrıları hiç izlenmez.
/// Sonsuz/çok-derin zincir riski böyle kapanır; "torun" notları KAPSAM DIŞI.
pub async fn list_visible_with_children(
    pool: &PgPool,
    wfd: &Wfd,
    wfe_id: Uuid,
    actor: &Actor,
) -> Result<Vec<NoteView>, AppError> {
    let mut notes = list_visible(pool, wfe_id, actor).await?;

    let calls = wf_wfe::repo::call::list_of_caller(pool, wfe_id)
        .await
        .map_err(db_err)?;
    for call in calls {
        let Some(callee_wfe_id) = call.callee_wfe_id else {
            continue;
        };
        let call_ref = match call.site_kind.as_str() {
            "terminal" => wfd
                .terminals
                .iter()
                .find(|t| t.id == call.site_key)
                .and_then(|t| t.call.as_ref()),
            _ => wfd.nodes.get(&call.site_key).and_then(|n| n.call.as_ref()),
        };
        let Some(call_ref) = call_ref else { continue };
        if !call_ref.notes_visible_to_caller {
            continue;
        }
        let mut child_notes = list_visible_published_only(pool, callee_wfe_id, actor).await?;
        for note in &mut child_notes {
            note.from_call = Some(call.call_key.clone());
        }
        notes.extend(child_notes);
    }
    notes.sort_by_key(|n| n.created_at);
    Ok(notes)
}

/// Verilen notların dosyalarını TEK sorguyla çeker (`list_visible` N+1
/// yapmasın diye, `count_by_wfe` deseninin aynısı). Gizli notlar da dahil
/// gelir — filtreleme `note_view`de `hidden_at`e göre yapılır, burada değil.
async fn files_for_notes(
    pool: &PgPool,
    note_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<NoteFileView>>, AppError> {
    if note_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, NoteFileRow>(&format!(
        "SELECT {NOTE_FILE_COLUMNS} FROM wf.wfe_note_file \
          WHERE note_id = ANY($1) ORDER BY created_at ASC"
    ))
    .bind(note_ids)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    let mut map: HashMap<Uuid, Vec<NoteFileView>> = HashMap::new();
    for r in rows {
        map.entry(r.note_id).or_default().push(NoteFileView::from(r));
    }
    Ok(map)
}

/// Nota dosya ekler (Faz 2, K3/K4). Kapılar `update_draft`/`hide` ile AYNI:
/// yazarı değilse `403`; not `draft` DEĞİLSE `409 code:"note.immutable"`.
/// Ardından limitler (spec "Ad-hoc dosya limitleri"): boş/aşırı büyük dosya
/// (`413 note.too_large`), blocklist MIME (`415 note.unsupported_type`), not
/// başına dosya sayısı ve WFE başına toplam kota (`422`). Dosya adı
/// `sanitize_filename` ile temizlenir. Yalnız DB satırını yazar — gerçek blob
/// yazımı ÇAĞIRAN TARAFTA (route), `AttachmentStore::note_write` ile olur;
/// blob yazımı başarısız olursa çağıran bu satırı `remove_file` ile geri alır.
pub async fn add_file(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
    filename: &str,
    mime: &str,
    size_bytes: i64,
) -> Result<Uuid, AppError> {
    let row = find_note(pool, wfe_id, note_id).await?;
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.status != "draft" {
        return Err(AppError {
            message: "yayınlanmış nota dosya eklenemez".into(),
            status: StatusCode::CONFLICT,
            code: Some("note.immutable"),
            items: None,
        });
    }
    if size_bytes <= 0 {
        return Err(AppError("boş dosya yüklenemez".into(), StatusCode::BAD_REQUEST));
    }
    if size_bytes > MAX_FILE_BYTES {
        return Err(AppError {
            message: format!(
                "dosya {} MB sınırını aşıyor",
                MAX_FILE_BYTES / 1024 / 1024
            ),
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: Some("note.too_large"),
            items: None,
        });
    }
    if is_blocked_mime(mime) {
        return Err(AppError {
            message: format!("izin verilmeyen içerik tipi: {mime}"),
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: Some("note.unsupported_type"),
            items: None,
        });
    }
    let existing_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM wf.wfe_note_file WHERE note_id = $1")
            .bind(note_id)
            .fetch_one(pool)
            .await
            .map_err(db_err)?;
    if existing_count >= MAX_FILES_PER_NOTE {
        return Err(AppError(
            format!("not başına en fazla {MAX_FILES_PER_NOTE} dosya eklenebilir"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    let used_bytes: i64 = sqlx::query_scalar(
        // `sum(bigint)` Postgres'te NUMERIC döner (taşma koruması) — `i64` decode'u
        // "mismatched types" ile patlar. Kota zaten `bigint` sınırının çok altında,
        // toplam güvenle geri daraltılır.
        "SELECT coalesce(sum(f.size_bytes), 0)::bigint FROM wf.wfe_note_file f \
           JOIN wf.wfe_note n ON n.note_id = f.note_id \
          WHERE n.wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_one(pool)
    .await
    .map_err(db_err)?;
    if used_bytes + size_bytes > MAX_WFE_QUOTA_BYTES {
        return Err(AppError(
            format!(
                "WFE başına toplam {} MB belge kotası aşıldı",
                MAX_WFE_QUOTA_BYTES / 1024 / 1024
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }

    let file_id = Uuid::new_v4();
    let clean_name = sanitize_filename(filename);
    // Kayıt amaçlı iz — gerçek anahtar `AttachmentStore::note_key` ile aynı
    // türetimden gelir (bkz. `NoteFileRow::storage_key` doc yorumu).
    let storage_key = format!("notes/{wfe_id}/{file_id}");
    sqlx::query(
        "INSERT INTO wf.wfe_note_file (file_id, note_id, filename, mime, size_bytes, storage_key) \
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(file_id)
    .bind(note_id)
    .bind(&clean_name)
    .bind(mime)
    .bind(size_bytes)
    .bind(&storage_key)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(file_id)
}

/// Nottan dosya siler — kapılar `add_file` ile AYNI (yazarı değilse `403`,
/// `draft` değilse `409 note.immutable`). Yalnız DB satırını siler; blob'un
/// storage'dan silinmesi ÇAĞIRAN TARAFTADIR (`AttachmentStore::note_delete`).
pub async fn remove_file(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    file_id: Uuid,
    actor: &Actor,
) -> Result<(), AppError> {
    let row = find_note(pool, wfe_id, note_id).await?;
    if !is_author(&row, actor) {
        return Err(AppError(
            "bu not size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    if row.status != "draft" {
        return Err(AppError {
            message: "yayınlanmış nottan dosya silinemez".into(),
            status: StatusCode::CONFLICT,
            code: Some("note.immutable"),
            items: None,
        });
    }
    let result = sqlx::query("DELETE FROM wf.wfe_note_file WHERE file_id = $1 AND note_id = $2")
        .bind(file_id)
        .bind(note_id)
        .execute(pool)
        .await
        .map_err(db_err)?;
    if result.rows_affected() == 0 {
        return Err(AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    Ok(())
}

/// Dosyayı WFE + NOT kapsamında arar (İNDİRME için) — `find_note` ile AYNI
/// IDOR gerekçesi: `file_id` tek başına anahtar DEĞİLDİR, daima
/// `(wfe_id, note_id, file_id)` üçlüsüyle çözülür; kapsam dışı `404`dür.
/// Görünürlük `assert_file_readable` ile (draft → yalnız yazarı, gizlenmiş
/// published → K3 gereği erişilemez) doğrulanır.
pub async fn find_file(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    file_id: Uuid,
    actor: &Actor,
) -> Result<NoteFileView, AppError> {
    let note = find_note(pool, wfe_id, note_id).await?;
    assert_file_readable(&note, actor)?;
    sqlx::query_as::<_, NoteFileRow>(&format!(
        "SELECT {NOTE_FILE_COLUMNS} FROM wf.wfe_note_file WHERE file_id = $1 AND note_id = $2"
    ))
    .bind(file_id)
    .bind(note_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .map(NoteFileView::from)
    .ok_or_else(|| AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND))
}

/// Verilen WFE'ler için görünür not sayısı (published + gizlenmemiş + `audience`
/// aktöre/yazara açık, K9) — TEK sorgu, N+1 yok (`wf_wfe::repo::wfah::max_seq_by_wfe`
/// deseni). Hiç görünür notu olmayan WFE haritada YER ALMAZ (çağıran 0'a düşer).
/// `actor` parametresi K9 öncesi yoktu — hedeflenmiş bir not "var" ama aktöre
/// görünmüyorsa, aktör boş listeye tıklardı; süzgeç `list_visible` ile AYNI olmalı.
pub async fn count_by_wfe(
    pool: &PgPool,
    wfe_ids: &[Uuid],
    actor: &Actor,
) -> Result<HashMap<Uuid, i64>, AppError> {
    if wfe_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let audience = audience_sql("", "$2", "$3");
    let rows = sqlx::query_as::<_, (Uuid, i64)>(&format!(
        "SELECT wfe_id, count(*) FROM wf.wfe_note \
          WHERE wfe_id = ANY($1) AND status = 'published' AND hidden_at IS NULL \
            AND {audience} \
          GROUP BY wfe_id"
    ))
    .bind(wfe_ids)
    .bind(actor.user_id)
    .bind(actor.orgu_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

/// Faz 3 (K9 okundu takibi): verilen WFE'ler için OKUNMAMIŞ görünür not sayısı
/// — `count_by_wfe` ile AYNI görünürlük süzgeci + `wf.wfe_note_read`'de bu
/// aktöre ait satırı OLMAYANLAR. Kendi yazdığın not hiç sayılmaz (yazma zaten
/// okumadır) — `NOT (author...)` ile dışlanır, LEFT JOIN'e güvenilmez çünkü
/// yazarın kendi notu için hiçbir zaman `wfe_note_read` satırı yazılmaz (bkz.
/// `mark_read`, yalnız başkasının notu işaretlenir). TEK sorgu, N+1 yok.
pub async fn unread_count_by_wfe(
    pool: &PgPool,
    wfe_ids: &[Uuid],
    actor: &Actor,
) -> Result<HashMap<Uuid, i64>, AppError> {
    if wfe_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let audience = audience_sql("n.", "$2", "$3");
    let rows = sqlx::query_as::<_, (Uuid, i64)>(&format!(
        "SELECT n.wfe_id, count(*) FROM wf.wfe_note n \
           LEFT JOIN wf.wfe_note_read r ON r.note_id = n.note_id AND r.user_id = $2 \
          WHERE n.wfe_id = ANY($1) AND n.status = 'published' AND n.hidden_at IS NULL \
            AND r.note_id IS NULL \
            AND NOT (n.author_user_id = $2 AND n.author_orgu_id = $3) \
            AND {audience} \
          GROUP BY n.wfe_id"
    ))
    .bind(wfe_ids)
    .bind(actor.user_id)
    .bind(actor.orgu_id)
    .fetch_all(pool)
    .await
    .map_err(db_err)?;
    Ok(rows.into_iter().collect())
}

/// Faz 3 (K9 okundu takibi): verilen notları bu AKTÖR için okunmuş işaretler
/// (`ON CONFLICT DO NOTHING` — tekrar çağrı no-op). Kapsam dışı `note_id`'ler
/// (başka bir WFE'ye ait, draft, ya da `audience` bu aktöre kapalı) SESSİZCE
/// atlanır — `INSERT ... SELECT` alt sorgusu onları hiç üretmez, kapsam kuralı
/// (K6/K9) `find_note`'daki IDOR gerekçesiyle AYNI: var olduğu da sızmaz.
/// Yazarın kendi notu için satır YAZILMAZ (`unread_count_by_wfe` zaten
/// `NOT author`la dışlıyor, gereksiz satır birikmesin).
pub async fn mark_read(
    pool: &PgPool,
    wfe_id: Uuid,
    note_ids: &[Uuid],
    actor: &Actor,
) -> Result<(), AppError> {
    if note_ids.is_empty() {
        return Ok(());
    }
    // Bind sırası: $1 wfe_id, $2 note_ids, $3 orgu_id, $4 user_id — `n.`
    // alias'ı JOIN'siz bu sorguda da (tekilleştirme amaçlı) tutulur, diğer
    // audience çağrılarıyla aynı desen okunsun diye.
    let audience = audience_sql("n.", "$4", "$3");
    sqlx::query(&format!(
        "INSERT INTO wf.wfe_note_read (note_id, user_id) \
         SELECT n.note_id, $4 FROM wf.wfe_note n \
          WHERE n.wfe_id = $1 AND n.note_id = ANY($2) \
            AND n.status = 'published' AND n.hidden_at IS NULL \
            AND NOT (n.author_user_id = $4 AND n.author_orgu_id = $3) \
            AND {audience} \
         ON CONFLICT DO NOTHING"
    ))
    .bind(wfe_id)
    .bind(note_ids)
    .bind(actor.orgu_id)
    .bind(actor.user_id)
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// K5: apply/action BAŞARILI olduktan SONRA çağrılır — WFAH'ın en son satırından
/// `seq`/`from_node` okunup nota çapa atılır (`node` geçişin ÖNCESİ adımıdır,
/// notun yazıldığı yer). Her iki route ağacı (`routes/wfe.rs::apply_action`,
/// `routes/portal/wfe.rs::submit_action`) aynı işlevi çağırır — SQL tek yerde
/// durur. Motor commit'i zaten gerçekleşti; burada hata olsa da apply sonucu
/// çağıran tarafta YİNE döner (not draft kalır, kullanıcı tekrar yayınlar).
pub async fn publish_after_apply(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
) -> Result<(), AppError> {
    let row = last_wfah(pool, wfe_id).await?;
    publish(
        pool,
        wfe_id,
        note_id,
        actor,
        Some(row.seq),
        row.from_node.as_deref(),
    )
    .await
}

/// WFE'nin EN SON defter satırı — `publish_after_apply`ın çapası ve
/// `republish_after_apply`ın kapısı aynı satırdan okunur.
#[derive(sqlx::FromRow)]
struct LastWfah {
    seq: i32,
    from_node: Option<String>,
    /// `wf.wfah.actor` jsonb'sinden çıkarılan `user_id` (sistem satırlarında nil UUID).
    actor_user_id: Option<Uuid>,
}

async fn last_wfah(pool: &PgPool, wfe_id: Uuid) -> Result<LastWfah, AppError> {
    sqlx::query_as::<_, LastWfah>(
        "SELECT seq, from_node, (actor->>'user_id')::uuid AS actor_user_id \
           FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?
    .ok_or_else(|| AppError("wfah kaydı bulunamadı".into(), StatusCode::INTERNAL_SERVER_ERROR))
}

/// Aksiyondan BAĞIMSIZ yayının (eski `POST .../publish`, çapasız not) yerini alan
/// TEK istisna (2026-08-11): apply BAŞARILI oldu ama nota çapa yazımı düştü
/// (`ApplyResult.note_error`) — aksiyon geri alınmadığı için not draft kaldı ve
/// kullanıcıya yeniden deneme yolu bırakılır.
///
/// Kapı: WFE'nin EN SON wfah satırı BU aktörün olmalı. Böylece uç "az önce aksiyon
/// alan kişinin, o aksiyona çapalanacak notu" ile sınırlı kalır; aksiyon almadan
/// not yayınlamanın yolu yoktur (409 `note.requires_action`). Çapa da o satırdır —
/// artık YAYINLANMIŞ HER NOT bir aksiyona bağlıdır (`wfah_seq` NULL olmaz).
///
/// Kabul edilen sınır: aynı aktör apply'dan sonra BAŞKA bir draft'ını da bu uçtan
/// yayınlayabilir (sunucu "hangi draft o apply'a gönderilmişti"yi bilmez — apply
/// yolu `note_id`'yi kalıcı olarak işaretlemiyor). Yayın yine gerçek bir aksiyona
/// çapalanır ve yine yalnız notun yazarı yapabilir; delil zincirinde boşluk açmaz.
pub async fn republish_after_apply(
    pool: &PgPool,
    wfe_id: Uuid,
    note_id: Uuid,
    actor: &Actor,
) -> Result<(), AppError> {
    let row = last_wfah(pool, wfe_id).await?;
    if row.actor_user_id != Some(actor.user_id) {
        return Err(AppError {
            message: "not yalnız bir aksiyonla yayınlanır — bu işte son aksiyon size ait değil"
                .into(),
            status: StatusCode::CONFLICT,
            code: Some("note.requires_action"),
            items: None,
        });
    }
    publish(
        pool,
        wfe_id,
        note_id,
        actor,
        Some(row.seq),
        row.from_node.as_deref(),
    )
    .await
}

/// K5 (2026-08-11 kuralı): not/dosya EKLEMEK claim ister. Not "bu adımı şu
/// gerekçeyle yaptım" kaydıdır ve yayınlanması aksiyona bağlıdır — işi üstlenmemiş
/// bir aktörün bıraktığı taslak hiçbir zaman yayınlanamaz (çapası olacak apply'ı o
/// aktör alamaz), 24 saat sonra süpürücüye kalırdı. Kapı `Engine::apply`'ın §7.1
/// assignment kontrolüyle AYNI soruyu sorar (`NotClaimed`/`NotOwner`): not
/// ekleyebilen, aksiyonu da alabilendir.
///
/// Paralel modda WFE-seviyesi `claimed_by` ANLAMSIZDIR (fork `current_node`'u
/// NULL'lar, claim kol-bazlıdır) — AKTİF kollardan en az biri bu aktörde olmalı.
/// Çağıran taraf zaten `executor.query` ile görünürlüğü doğruladı (K6); bu kapı
/// onun ÜSTÜNE biner, yerine geçmez.
pub fn assert_actor_holds_claim(
    view: &wf_wfe::executor::WfeView,
    actor: &Actor,
) -> Result<(), AppError> {
    let active_branch_claimers: Vec<Uuid> = view
        .branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
        .filter_map(|b| b.claimed_by)
        .collect();
    let holds = holds_claim(
        view.join_target.is_some(),
        view.claimed_by,
        &active_branch_claimers,
        actor.user_id,
    );
    if holds {
        return Ok(());
    }
    Err(AppError {
        message: "not/dosya eklemek için işi üstüne almalısınız (claim)".into(),
        status: StatusCode::CONFLICT,
        code: Some("note.requires_claim"),
        items: None,
    })
}

/// `assert_actor_holds_claim`ın SAF çekirdeği — `WfeView` kurmadan sınanabilsin diye
/// ayrı (bu repoda DB'li test koşulmuyor, karar kuralı yine de test altında olmalı).
fn holds_claim(
    parallel: bool,
    wfe_claimed_by: Option<Uuid>,
    active_branch_claimers: &[Uuid],
    user_id: Uuid,
) -> bool {
    if parallel {
        active_branch_claimers.contains(&user_id)
    } else {
        wfe_claimed_by == Some(user_id)
    }
}

#[cfg(test)]
mod tests {
    use super::holds_claim;
    use uuid::Uuid;

    #[test]
    fn single_branch_requires_the_wfe_claim() {
        let me = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        assert!(holds_claim(false, Some(me), &[], me));
        assert!(!holds_claim(false, None, &[], me), "claim yoksa not eklenemez");
        assert!(!holds_claim(false, Some(other), &[], me), "claim başkasında");
    }

    #[test]
    fn parallel_mode_ignores_wfe_level_claim() {
        let me = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        // Fork'ta WFE-seviyesi claim ANLAMSIZDIR — kol claim'i olmadan kapı kapalı.
        assert!(!holds_claim(true, Some(me), &[], me));
        assert!(holds_claim(true, None, &[other, me], me), "aktif kollardan biri bende");
        assert!(!holds_claim(true, None, &[other], me));
    }
}

/// Yetim draft'ları süpürür (K5, TTL 24 saat) — kullanıcı yayınlamadan/silmeden
/// vazgeçtiği taslaklar sonsuza kalmaz. Faz 2: draft'ın DOSYALARI da temizlenir —
/// DB satırı `ON DELETE CASCADE` ile gider ama storage'daki blob kalıcı olur,
/// onu burada `AttachmentStore::note_delete` ile siliyoruz. Bu yüzden `&PgPool`
/// yerine `&AppState` alır (depo çözümü `attachment_store::store_for_wfe_strict`
/// WFD'nin `$env`'ine bakar, `state.pool` üzerinden de sorgu yapar).
///
/// Sıra: önce (varsa) dosyalar best-effort silinir (`reservation::sweep`'teki
/// "önce dosya, sonra satır" gerekçesinin aynısı — ters sırada satır silinip
/// dosya silme başarısız olsaydı blob'un kime ait olduğu bir daha bilinemezdi),
/// sonra draft satırları tek DELETE ile silinir (CASCADE `wfe_note_file`
/// satırlarını temizler). Bir WFE/depo çözülemezse (örn. `$env` artık eksik)
/// o WFE'nin dosyaları warn ile atlanır, döngü DURMAZ — süpürücü best-effort'tur.
pub async fn sweep_expired_drafts(state: &crate::state::AppState) -> Result<u64, AppError> {
    let pool = &state.pool;
    let expired: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!(
        "SELECT note_id, wfe_id FROM wf.wfe_note \
          WHERE status = 'draft' AND created_at < now() - interval '{DRAFT_TTL_HOURS} hours'"
    ))
    .fetch_all(pool)
    .await
    .map_err(db_err)?;

    for (note_id, wfe_id) in &expired {
        let file_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT file_id FROM wf.wfe_note_file WHERE note_id = $1")
                .bind(note_id)
                .fetch_all(pool)
                .await
                .map_err(db_err)?;
        if file_ids.is_empty() {
            continue;
        }
        let store = match crate::attachment_store::store_for_wfe_strict(state, *wfe_id).await {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(
                    wfe_id = %wfe_id, note_id = %note_id,
                    "taslak not deposu çözülemedi, dosyalar atlandı: {}", e.message
                );
                continue;
            }
        };
        for file_id in file_ids {
            if let Err(e) = store.note_delete(*wfe_id, file_id).await {
                tracing::warn!(
                    wfe_id = %wfe_id, %file_id,
                    "taslak not dosyası silinemedi: {e}"
                );
            }
        }
    }

    let result = sqlx::query(&format!(
        "DELETE FROM wf.wfe_note \
          WHERE status = 'draft' AND created_at < now() - interval '{DRAFT_TTL_HOURS} hours'"
    ))
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(result.rows_affected())
}
