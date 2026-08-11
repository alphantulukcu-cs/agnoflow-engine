//! Portal ek-belge (attachment) endpoint'leri — upload / download / delete +
//! aksiyon-gate yardımcıları.
//!
//! Mimari: engine core (wfe-core) dosya I/O yapmaz; yalnız WFD attachment
//! katalogunu ve node referanslarını metadata olarak taşır. Dosya varlığı ve
//! yükleme bu PORTAL katmanında `AttachmentStore` (opendal) üzerinden yürür.
//! `nodes[x].attachments` bir grup listesi referanslar; grup içindeki `required`
//! item'lar yüklenmeden o node'dan hiçbir aksiyon submit edilemez (gate hem
//! `get_wfe_detail`'de bilgi, hem `submit_action`'da zorlama olarak uygulanır).

use utoipa_axum::router::OpenApiRouter;
use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use utoipa_axum::routes;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::ports::WfdStore;

// Durum tipleri + gate yardımcıları paylaşımlıdır (bkz. crate::attachments) — hem bu
// JWT route ağacı hem direkt X-Actor route ağacı (routes/attachments.rs) aynısını kullanır.
//
// `enrich_with_meta`: `GET /wfe/:id/attachments`in JWT karşılığı bu dosyada AYRI bir
// handler olarak yok — `wf.wfe_attachment` görünürlüğü `routes/portal/wfe.rs`deki
// WFE detay ucuna (`status_for_node` zaten oradan çağrılıyor) gömülüdür. Aynı cevabı
// üretmek isteyen o kod burada re-export edilen fonksiyonu kullanır; mantık burada
// (crate::attachments) tek kopya kalır, iki ağaç ayrı davranmaz.
pub use crate::attachments::{
    enrich_with_meta, missing_required, satisfied, status_for_node, AttachmentGroupStatus,
};

/// wfe router'ına merge edilir (aynı `/:wfe_id` uzayında). State merge'den sonra bağlanır.
pub fn routes() -> OpenApiRouter<AppState> {
    // DİKKAT: `routes!` TEK path'e TEK MethodRouter kurar (path'i ilk handler'dan alır).
    // `upload` (`/{wfe_id}/attachments/{group}/{item}`) ile `upload_multi`
    // (`/{wfe_id}/attachments`) aynı makroya konursa ikisi de AYNI yola PUT olarak
    // bağlanır ve axum açılışta "Overlapping method route" ile PANİKLER. Farklı path'ler
    // AYRI `.routes(...)` çağrısı ister.
    OpenApiRouter::new()
        .routes(routes!(download, upload, remove))
        .routes(routes!(upload_multi))
}

/// `PortalActor` (JWT) → `Actor` (engine/paylaşımlı yardımcıların ortak tipi). Bu ağacın
/// diğer dosyaları (`routes/portal/wfe.rs::to_actor`) da AYNI dönüşümü kendi kopyasında
/// yapar — o fonksiyon PRIVATE, buraya taşınamaz; alan eşlemesi birebir aynı kalmalı.
fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

// ---- WFD çözümü (orgtnt sahipliği + aktif WFE doğrulaması) ----

#[derive(sqlx::FromRow)]
struct WfeWfdRow {
    wfd_id: Uuid,
    wfd_version: i32,
}

/// WFE'nin aktör orgtnt'sine ait ve aktif olduğunu doğrular, WFD'yi getirir.
pub async fn load_wfd_for_wfe(
    s: &AppState,
    wfe_id: Uuid,
    orgtnt_id: Uuid,
) -> Result<Wfd, AppError> {
    let row = sqlx::query_as::<_, WfeWfdRow>(
        "SELECT e.wfd_id, e.wfd_version
         FROM wf.wfe e
         WHERE e.wfe_id = $1 AND e.orgtnt_id = $2 AND e.status = 'active'",
    )
    .bind(wfe_id)
    .bind(orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    s.wfd
        .fetch(row.wfd_id, row.wfd_version)
        .await
        .map_err(AppError::from)
}

/// Katalogda grup+item gerçekten var mı? Yoksa 404 (rastgele anahtara yazmayı engeller).
fn find_item<'a>(
    wfd: &'a Wfd,
    group: &str,
    item: &str,
) -> Result<&'a wfe_core::types::wfd_v22::AttachmentItem, AppError> {
    wfd.attachments
        .get(group)
        .and_then(|g| g.items.iter().find(|i| i.id == item))
        .ok_or_else(|| {
            AppError(
                format!("attachment slotu bulunamadı: {group}/{item}"),
                StatusCode::NOT_FOUND,
            )
        })
}

// ---- handler'lar ----

#[utoipa::path(put,
    operation_id = "portal_attachment_upload", path = "/{wfe_id}/attachments/{group}/{item}", tag = "attachments",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("group" = String, Path, description = "Attachment grup key"),
        ("item" = String, Path, description = "Grup içi item id")),
    request_body = Vec<u8>,
    responses((status = 200, description = "Yüklendi", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn upload(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<serde_json::Value>, AppError> {
    let wfd = load_wfd_for_wfe(&s, wfe_id, actor.orgtnt_id).await?;
    let def = find_item(&wfd, &group, &item)?;

    // Boş dosya reddedilir (exists gate'i anlamlı kalsın).
    if body.is_empty() {
        return Err(AppError(
            "boş dosya yüklenemez".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    // Format kuralları (tip + o gruba özel boyut) — paylaşımlı doğrulayıcı.
    let ct = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or("").trim());
    match crate::attachments::check_upload(def, ct, body.len()) {
        Ok(()) => {}
        Err(crate::attachments::UploadReject::UnsupportedType(ct)) => {
            return Err(AppError(
                format!("izin verilmeyen içerik tipi: {ct}"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ))
        }
        Err(crate::attachments::UploadReject::TooLarge(max_mb)) => {
            return Err(AppError(
                format!("dosya {max_mb} MB sınırını aşıyor"),
                StatusCode::PAYLOAD_TOO_LARGE,
            ))
        }
        // Magic-byte çelişkisi — direkt `/wfe/*` ağacındaki kardeşiyle aynı statü ve metin
        // (bkz. routes/attachments.rs::validate_upload); iki ağaç aynı cevabı vermeli.
        Err(crate::attachments::UploadReject::TypeMismatch { declared, detected }) => {
            return Err(AppError(
                format!("içerik beyan edilen tiple uyuşmuyor: {declared} denildi, {detected} bulundu"),
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
            ))
        }
    }

    // YAZMA yolu KATIDIR: `$env`de depo tanımlı değilse sunucu diskine düşmek yerine
    // 422. Gerekçe direkt ağaçtaki kardeşiyle aynı (bkz. routes/attachments.rs::resolve_target);
    // indirme/silme fallback'i korur, eski dosyalar erişilebilir kalsın.
    crate::attachment_store::store_for_wfe_strict(&s, wfe_id)
        .await?
        .write(wfe_id, &group, &item, body.to_vec())
        .await
        .map_err(|e| {
            AppError(
                format!("yükleme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;

    Ok(Json(serde_json::json!({
        "uploaded": true,
        "group": group,
        "item": item
    })))
}

#[utoipa::path(get,
    operation_id = "portal_attachment_download", path = "/{wfe_id}/attachments/{group}/{item}", tag = "attachments",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("group" = String, Path, description = "Attachment grup key"),
        ("item" = String, Path, description = "Grup içi item id")),
    responses((status = 200, description = "Dosya içeriği (application/octet-stream)")),
    security(("bearer_jwt" = [])))]
async fn download(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let wfd = load_wfd_for_wfe(&s, wfe_id, actor.orgtnt_id).await?;
    find_item(&wfd, &group, &item)?;

    let bytes = crate::attachment_store::store_for_wfe(&s, wfe_id)
        .await?
        .read(wfe_id, &group, &item)
        .await
        .map_err(|_| AppError("dosya bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut h = HeaderMap::new();
    h.insert(
        axum::http::header::CONTENT_TYPE,
        "application/octet-stream".parse().unwrap(),
    );
    Ok((StatusCode::OK, h, Bytes::from(bytes)))
}

#[utoipa::path(delete,
    operation_id = "portal_attachment_remove", path = "/{wfe_id}/attachments/{group}/{item}", tag = "attachments",
    params(
        ("wfe_id" = Uuid, Path, description = "WFE id"),
        ("group" = String, Path, description = "Attachment grup key"),
        ("item" = String, Path, description = "Grup içi item id")),
    responses((status = 204, description = "Silindi")),
    security(("bearer_jwt" = [])))]
async fn remove(
    State(s): State<AppState>,
    actor: PortalActor,
    Path((wfe_id, group, item)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, AppError> {
    let wfd = load_wfd_for_wfe(&s, wfe_id, actor.orgtnt_id).await?;
    find_item(&wfd, &group, &item)?;

    crate::attachment_store::store_for_wfe(&s, wfe_id)
        .await?
        .delete(wfe_id, &group, &item)
        .await
        .map_err(|e| {
            AppError(
                format!("silme başarısız: {e}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )
        })?;
    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /{wfe_id}/attachments` — çok dosyalı, AKSİYONSUZ yükleme, JWT ağacının ince kabuğu.
/// Ortak mantık (staging → doğrulama → atomik promote) `crate::routes::attachments::
/// upload_multi_shared`de TEK yerde tutulur; bu ağaç yalnız `PortalActor`ı `Actor`a çevirip
/// çağırır — iki ağaç (X-Actor / JWT) AYNI cevabı vermeli.
#[utoipa::path(put,
    operation_id = "portal_attachment_upload_multi", path = "/{wfe_id}/attachments", tag = "attachments",
    params(("wfe_id" = Uuid, Path, description = "WFE id")),
    request_body(content = String, description = "multipart/form-data — alan adları `{grup}/{slot}`; `payload` part'ı YOK (aksiyonsuz)"),
    responses(
        (status = 200, description = "Yükleme sonucu", body = serde_json::Value),
        (status = 422, description = "Bir veya daha fazla dosya reddedildi (attachment.rejected)"),
    ),
    security(("bearer_jwt" = [])))]
async fn upload_multi(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    mp: Multipart,
) -> Result<Json<crate::routes::attachments::UploadMultiResponse>, AppError> {
    let actor = to_actor(&actor);
    crate::routes::attachments::upload_multi_shared(&s, &actor, wfe_id, mp)
        .await
        .map(Json)
}
