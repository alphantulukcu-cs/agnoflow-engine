//! `POST /uploads` — yükleme STAGING alanı (2026-08-11, K8 / Faz 3).
//!
//! Tasarım: `docs/superpowers/specs/2026-08-11-tek-istekte-baslatma-design.md`,
//! "K8" ve "`POST /uploads` — staging (Faz 3)" bölümleri; asıl iş mantığı
//! `crate::staging`'de (bu dosya yalnız HTTP kabuğudur — `routes/attachments.rs`'in
//! `crate::attachments`'a oranı gibi).
//!
//! Akış:
//! ```text
//! POST /uploads {wfd_id, version, group, item, environment?} → {upload_id, url?, expires_at}
//! PUT  <url>                                                  → S3'e DOĞRUDAN (local'de sunucuya)
//! POST /wfe  payload.attachments[].upload_id                  → crate::staging::take (HEAD + server-side COPY)
//! ```
//!
//! `url` yalnız backend PRESIGN destekliyorsa (S3) dolar; local backend presign
//! desteklemez → `None`, istemci bu durumda `PUT /uploads/{upload_id}` kullanır.
//! Sunucu bunu backend adına (`StorageBackend::S3` vb.) bakarak DEĞİL, `Operator`ın
//! kendi bildirdiği yetenekten (`Operator::info().full_capability().presign_write`)
//! anlar — böylece staging kodu depo seçimine dair varsayım yapmaz, hangi backend
//! presign destekliyorsa (bugün yalnız S3) otomatik ondan yararlanır.

use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::v22::ports::WfdStore;

/// Presigned URL'nin / doğrudan `PUT` penceresinin geçerlilik süresi (K8: "Süre 1 saat").
const UPLOAD_TTL_SECS: u64 = 3600;

/// `PUT /uploads/{id}` gövdesi için sert üst sınır — `ATTACHMENT_MAX_REQUEST_MB`
/// (varsayılan 200) yeniden kullanılır. Bu rota `/wfe`+`/portal` alt ağaçlarının
/// `DefaultBodyLimit` layer'ının KAPSAMINDA DEĞİLDİR (bkz. dosya sonu rapor notu —
/// main.rs'e aynı layer'ın eklenmesi gerekir); gövde burada MANUEL stream edildiği
/// için `DefaultBodyLimit` zaten devreye girmez — bu yüzden aynı tavan STREAM
/// döngüsünün İÇİNDE de zorlanır (aşağıda `put_upload`), main.rs'teki layer eksik
/// kalsa bile bu handler kendi başına sınırsız yüklemeyi kabul ETMEZ.
fn max_upload_bytes(s: &AppState) -> u64 {
    s.cfg.attachment_max_request_mb * 1024 * 1024
}

/// `/uploads` altında merge edilir.
pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(create_upload))
        .routes(routes!(put_upload))
        .routes(routes!(delete_upload))
        .with_state(state)
}

#[derive(Deserialize, ToSchema)]
struct CreateUploadBody {
    wfd_id: Uuid,
    version: i32,
    group: String,
    item: String,
    /// Koşum ortamı ADI — `POST /wfe`deki `environment` ile AYNI sözleşme: depo bu
    /// ortamın `$env`inden çözülür (`store_for_wfd`), verilmezse tenant varsayılanı.
    /// `take()` NİHAİ anahtara taşırken AYNI ortamı kullanır — staging ile hedef farklı
    /// depoda olursa server-side copy imkânsızlaşır (bkz. `crate::staging` başlığı).
    #[serde(default)]
    environment: Option<String>,
}

#[derive(Serialize, ToSchema)]
struct CreateUploadResult {
    upload_id: Uuid,
    /// S3 backend'inde presigned `PUT` URL'i; local backend presign DESTEKLEMEZ →
    /// `None` — istemci `PUT /uploads/{upload_id}` kullanır.
    url: Option<String>,
    expires_at: DateTime<Utc>,
}

/// `wfd.attachments`ta bu grup/item gerçekten var mı? Yoksa `take()` hiçbir zaman
/// başarılı olamaz — dosya TTL boyunca boşuna staging'de beklerdi; erken 404 ucuzdur.
/// `routes/attachments.rs::find_item` ile AYNI soruyu sorar ama o fonksiyon PRIVATE
/// ve o dosyaya bu görevde dokunulamıyor — küçük olduğu için burada yeniden yazıldı.
fn assert_slot_exists(
    wfd: &wfe_core::types::wfd_v22::Wfd,
    group: &str,
    item: &str,
) -> Result<(), AppError> {
    let ok = wfd
        .attachments
        .get(group)
        .is_some_and(|g| g.items.iter().any(|i| i.id == item));
    if ok {
        Ok(())
    } else {
        Err(AppError(
            format!("attachment slotu bulunamadı: {group}/{item}"),
            StatusCode::NOT_FOUND,
        ))
    }
}

/// Ortam ADINI tenant'ın kayıtlı ortamlarına çözer. `routes/wfe.rs::resolve_environment_id`
/// ile AYNI mantık (o fonksiyon PRIVATE, bu dosyaya dokunma izni yok) — küçük olduğu
/// için burada bağımsız bir kopyası tutulur; ikisi de nihayetinde AYNI
/// `wf_wfe::repo::env::resolve_environment`i çağırır, sözleşme tek yerde (orada) yaşar.
async fn resolve_environment_id(
    s: &AppState,
    orgtnt_id: Uuid,
    name: Option<&str>,
) -> Result<Option<Uuid>, AppError> {
    let Some(name) = name else { return Ok(None) };
    wf_wfe::repo::env::resolve_environment(&s.pool, orgtnt_id, Some(name))
        .await
        .map(|e| Some(e.id))
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))
}

/// Staging tutamağı üretir. Yetki `assert_can_start` ile AYNI kapı (K8: staging yalnız
/// BAŞLATMA aksiyonu için tanımlıdır — Faz 3 kapsamı) — `reserve_wfe`nin yaptığı erken
/// yetki kontrolünün aynısı, gerekçesi de aynı: yetkisiz aktörün dosyaları depoya hiç
/// değmesin (`upload_id` bile ALAMASIN).
///
/// NOT: `action` bu gövdede YOK (K8 sözleşmesi `{wfd_id, version, group, item,
/// environment?}`) — `reserve_wfe`nin `action: None` verildiği hâliyle aynı: herhangi
/// bir start kuralının izin vermesi yeterli.
#[utoipa::path(post, path = "/", tag = "uploads",
    request_body = CreateUploadBody,
    responses(
        (status = 200, description = "Staging handle'ı üretildi", body = CreateUploadResult),
        (status = 403, description = "Aktör bu akışı başlatamaz"),
        (status = 404, description = "group/item katalogda yok"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn create_upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateUploadBody>,
) -> Result<Json<CreateUploadResult>, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;

    // WFD gerçekten var mı (ve bu sürüm çekilebiliyor mu) — `reserve_wfe` ile AYNI
    // gerekçe: staging uydurma bir dokümana/gruba bağlanmasın.
    let wfd = s
        .wfd
        .fetch(body.wfd_id, body.version)
        .await
        .map_err(AppError::from)?;
    super::wfe::assert_can_start(&s, &wfd, &actor, orgtnt_id, None).await?;
    assert_slot_exists(&wfd, &body.group, &body.item)?;

    let environment_id = resolve_environment_id(&s, orgtnt_id, body.environment.as_deref()).await?;
    let upload_id = Uuid::new_v4();

    // Depo NİHAİ anahtarla AYNI olmalı (bkz. `crate::staging` modül başlığı) — bu
    // yüzden `store_for_wfd` burada da, `take()`te de AYNI (wfd_id, environment_id)
    // çiftiyle çağrılır.
    let store =
        crate::attachment_store::store_for_wfd_strict(&s, body.wfd_id, orgtnt_id, environment_id)
            .await?;
    // bkz. `crate::staging` modül başlığı: `AttachmentStore::operator()` henüz YOK.
    let op = store.operator();

    let staging_key = crate::staging::staging_key(upload_id);
    // Presign SADECE backend gerçekten destekliyorsa denenir (bugün yalnız S3) —
    // `StorageBackend` enum'ına bakmak yerine `Operator`ın kendi yetenek beyanına
    // güvenilir (bkz. dosya başlığı).
    let url = if op.info().full_capability().presign_write {
        match op
            .presign_write(
                &staging_key,
                std::time::Duration::from_secs(UPLOAD_TTL_SECS),
            )
            .await
        {
            Ok(signed) => Some(signed.uri().to_string()),
            // Presign servis/yetki hatası: sessizce PUT yoluna düş — istemci
            // `PUT /uploads/{id}` kullanır, akış durmaz (presign yalnız bir KOLAYLIK,
            // local yol her zaman çalışır).
            Err(e) => {
                tracing::warn!("presign_write başarısız, PUT yoluna düşülüyor: {e}");
                None
            }
        }
    } else {
        None
    };

    let created_at = Utc::now();
    let staged = crate::staging::Staged {
        upload_id,
        orgtnt_id,
        wfd_id: body.wfd_id,
        wfd_version: body.version,
        environment_id,
        grp: body.group,
        item: body.item,
        actor_orgu_id: actor.orgu_id,
        actor_user_id: actor.user_id,
        created_at,
    };
    crate::staging::create(&s.pool, &staged).await?;

    Ok(Json(CreateUploadResult {
        upload_id,
        url,
        expires_at: created_at + Duration::seconds(UPLOAD_TTL_SECS as i64),
    }))
}

/// Sahiplik + varlık ortak çözümü — `PUT`/`DELETE` ikisi de kullanır.
async fn load_owned(
    s: &AppState,
    actor: &Actor,
    upload_id: Uuid,
) -> Result<crate::staging::Staged, AppError> {
    let staged = crate::staging::get(&s.pool, upload_id)
        .await?
        .ok_or_else(|| AppError("upload bulunamadı".into(), StatusCode::NOT_FOUND))?;
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    if !crate::staging::owned_by(&staged, orgtnt_id, actor) {
        return Err(AppError(
            "bu yükleme size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }
    Ok(staged)
}

/// Local yol: gövdeyi staging anahtarına STREAM'le yazar. `axum::body::Bytes`
/// extractor'ı KULLANILMAZ — o tüm gövdeyi tek seferde belleğe alır, K8'in bütün
/// gerekçesi (500 MB'lık bir dosyanın engine/sunucu belleğinden GEÇMEMESİ) bu adımda
/// boşa çıkardı. Bunun yerine ham gövde `Body::into_data_stream()` ile parça parça
/// okunur, her parça doğrudan `Operator::writer`a yazılır.
#[utoipa::path(put, path = "/{upload_id}", tag = "uploads",
    params(("upload_id" = Uuid, Path, description = "Staging handle (`POST /uploads`in döndürdüğü id)")),
    request_body(content = Vec<u8>, description = "Dosya içeriği (binary), stream yazılır"),
    responses(
        (status = 200, description = "Yükleme sonucu", body = serde_json::Value),
        (status = 403, description = "Bu yükleme size ait değil"),
        (status = 404, description = "upload_id bulunamadı"),
        (status = 413, description = "Gövde ATTACHMENT_MAX_REQUEST_MB sınırını aşıyor"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn put_upload(
    State(s): State<AppState>,
    Path(upload_id): Path<Uuid>,
    req: axum::extract::Request,
) -> Result<Json<serde_json::Value>, AppError> {
    let headers = req.headers().clone();
    let actor = super::wfe::extract_actor(&headers)?;
    let staged = load_owned(&s, &actor, upload_id).await?;

    let store = crate::attachment_store::store_for_wfd_strict(
        &s,
        staged.wfd_id,
        staged.orgtnt_id,
        staged.environment_id,
    )
    .await?;
    let op = store.operator();
    let key = crate::staging::staging_key(upload_id);

    // İstemcinin bildirdiği Content-Type — S3'te nesne metadata'sı olarak taşınır ki
    // `take()`in `stat()`le okuduğu `content_type` boş kalmasın (local Fs backend'de
    // zaten hiçbir zaman desteklenmez, `writer_with` bu durumda sessizce yok sayar).
    let declared_ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty());

    let mut writer = match &declared_ct {
        Some(ct) => op.writer_with(&key).content_type(ct).await,
        None => op.writer(&key).await,
    }
    .map_err(|e| {
        AppError(
            format!("yükleme başlatılamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    // `futures_util::StreamExt::next` ile parça parça okunur — bkz. dosya sonu rapor
    // notu: `futures-util` bu görevde `crates/server/Cargo.toml`a EKLENMESİ gereken
    // tek yeni bağımlılıktır (Cargo.lock'ta zaten 0.3.32 olarak transitif mevcut,
    // axum/opendal onu zaten kullanıyor — yeni bir sürüm çakışması yaratmaz).
    use futures_util::StreamExt;
    let max_bytes = max_upload_bytes(&s);
    let mut stream = req.into_body().into_data_stream();
    let mut total: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|e| AppError(format!("gövde akışı kesildi: {e}"), StatusCode::BAD_REQUEST))?;
        total += chunk.len() as u64;
        if total > max_bytes {
            // Yarım nesne TAMAMLANMASIN (S3'te multipart upload abort edilir) —
            // `routes/wfe.rs::start_multipart`teki aynı desen (`close` ETMEDEN `abort`).
            let _ = writer.abort().await;
            return Err(AppError(
                format!(
                    "istek {} MB (ATTACHMENT_MAX_REQUEST_MB) sınırını aşıyor",
                    s.cfg.attachment_max_request_mb
                ),
                StatusCode::PAYLOAD_TOO_LARGE,
            ));
        }
        writer.write(chunk).await.map_err(|e| {
            AppError(
                format!("yükleme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    }
    writer.close().await.map_err(|e| {
        AppError(
            format!("yükleme kapatılamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;

    Ok(Json(json!({
        "uploaded": true,
        "upload_id": upload_id,
        "size_bytes": total,
    })))
}

/// İstemci vazgeçti (ya da yükleme yarıda kaldı) — staging nesnesi + satırı silinir.
/// IDEMPOTENT: kayıt yoksa (süpürülmüş, zaten silinmiş, ya da `take()` ile zaten
/// nihai konumuna taşınmış) yine 204 — `DELETE /wfe/reserve/{id}` ile AYNI sözleşme
/// (bkz. `crate::reservation`'ın doc yorumu).
#[utoipa::path(delete, path = "/{upload_id}", tag = "uploads",
    params(("upload_id" = Uuid, Path, description = "Staging handle")),
    responses(
        (status = 204, description = "Bırakıldı (nesne + kayıt silindi, ya da zaten yoktu)"),
        (status = 403, description = "Bu yükleme size ait değil"),
    ),
    security(("x_actor_orgu" = []), ("x_actor_user" = []), ("x_actor_role" = [])))]
async fn delete_upload(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(upload_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let actor = super::wfe::extract_actor(&headers)?;
    let Some(staged) = crate::staging::get(&s.pool, upload_id).await? else {
        return Ok(StatusCode::NO_CONTENT);
    };
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    if !crate::staging::owned_by(&staged, orgtnt_id, &actor) {
        return Err(AppError(
            "bu yükleme size ait değil".into(),
            StatusCode::FORBIDDEN,
        ));
    }

    let store =
        crate::attachment_store::store_for_wfd(&s, staged.wfd_id, orgtnt_id, staged.environment_id)
            .await?;
    let op = store.operator();
    // Nesne silme idempotent (opendal semantiğinde yoksa da hata vermez) — bu yüzden
    // "önce nesne sonra satır" sırası burada da güvenle uygulanabilir (`take`/
    // `sweep_expired`teki AYNI gerekçe: satır kalıp nesne silinemezse süpürücü/istemci
    // tekrar dener; tersi olsaydı sahipsiz kalan nesne asla bulunamazdı).
    op.delete(&crate::staging::staging_key(upload_id))
        .await
        .map_err(|e| {
            AppError(
                format!("staging nesnesi silinemedi: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    crate::staging::delete(&s.pool, upload_id).await?;
    Ok(StatusCode::NO_CONTENT)
}
