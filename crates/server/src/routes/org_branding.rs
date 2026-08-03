//! Tenant marka varlığı endpoint'leri — ADMIN ağacı (`/org/orgtnt/{id}/logo/{slot}`,
//! X-Admin-Key). Ayarlar > Organizasyon ekranı bu rotaları kullanır.
//!
//! Depolama/doğrulama mantığı `crate::branding`'de; portal ağacı (JWT, salt okuma)
//! aynı fonksiyonları `routes/portal/branding.rs` üzerinden çağırır. Bayt'lar WFD
//! JSON'la aynı tenant-prefixli bucket'ta, `{orgtnt_id}/logo/` altında yaşar.

use crate::branding::{self, BrandingSummary};
use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use utoipa_axum::{router::OpenApiRouter, routes};
use uuid::Uuid;
use wf_org::{models::Orgtnt, repo};

/// `/org` router'ına merge edilir (aynı X-Admin-Key kapısının arkasında).
pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(get_branding))
        .routes(routes!(download, upload, remove))
        .with_state(state)
}

/// İstek gövdesindeki `Content-Type` — parametreler atılır (`; charset=...`).
fn content_type(headers: &HeaderMap) -> &str {
    headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        .unwrap_or("")
}

async fn load_tenant(s: &AppState, id: Uuid) -> Result<Orgtnt, AppError> {
    repo::orgtnt::get(&s.pool, id).await.map_err(Into::into)
}

#[utoipa::path(get, path = "/orgtnt/{id}/branding", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")),
    responses((status = 200, description = "Marka özeti (bayt taşımaz)", body = BrandingSummary)),
    security(("x_admin_key" = [])))]
async fn get_branding(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<BrandingSummary>, AppError> {
    let tenant = load_tenant(&s, id).await?;
    Ok(Json(BrandingSummary::from(&tenant)))
}

#[utoipa::path(put, path = "/orgtnt/{id}/logo/{slot}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("slot" = String, Path, description = "Varlık slotu: logo | favicon"),
    ),
    request_body(content = Vec<u8>, description = "Görsel içeriği (binary). \
        logo: png/jpeg/webp/svg ≤2 MB — favicon: ek olarak ico ≤512 KB"),
    responses(
        (status = 200, description = "Güncellenen marka özeti", body = BrandingSummary),
        (status = 413, description = "Boyut sınırı aşıldı"),
        (status = 415, description = "İzin verilmeyen içerik tipi"),
    ),
    security(("x_admin_key" = [])))]
async fn upload(
    State(s): State<AppState>,
    Path((id, slot)): Path<(Uuid, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<BrandingSummary>, AppError> {
    let slot = branding::parse_slot(&slot)?;
    let tenant = load_tenant(&s, id).await?;
    let mime = content_type(&headers).to_string();
    let updated = branding::store(&s.wfd.storage, &s.pool, &tenant, slot, &mime, body).await?;
    Ok(Json(BrandingSummary::from(&updated)))
}

#[utoipa::path(get, path = "/orgtnt/{id}/logo/{slot}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("slot" = String, Path, description = "Varlık slotu: logo | favicon"),
    ),
    responses(
        (status = 200, description = "Görsel içeriği", body = Vec<u8>),
        (status = 404, description = "Slot boş"),
    ),
    security(("x_admin_key" = [])))]
async fn download(
    State(s): State<AppState>,
    Path((id, slot)): Path<(Uuid, String)>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let slot = branding::parse_slot(&slot)?;
    let tenant = load_tenant(&s, id).await?;
    let (h, bytes) = branding::read(&s.wfd.storage, &tenant, slot).await?;
    Ok((StatusCode::OK, h, bytes))
}

#[utoipa::path(delete, path = "/orgtnt/{id}/logo/{slot}", tag = "org",
    params(
        ("id" = Uuid, Path, description = "Tenant id"),
        ("slot" = String, Path, description = "Varlık slotu: logo | favicon"),
    ),
    responses((status = 200, description = "Güncellenen marka özeti", body = BrandingSummary)),
    security(("x_admin_key" = [])))]
async fn remove(
    State(s): State<AppState>,
    Path((id, slot)): Path<(Uuid, String)>,
) -> Result<Json<BrandingSummary>, AppError> {
    let slot = branding::parse_slot(&slot)?;
    let tenant = load_tenant(&s, id).await?;
    let updated = branding::remove(&s.wfd.storage, &s.pool, &tenant, slot).await?;
    Ok(Json(BrandingSummary::from(&updated)))
}
