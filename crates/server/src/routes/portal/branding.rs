//! Portal marka endpoint'leri — JWT ağacı, SALT OKUMA.
//!
//! Tenant token'dan çözülür (id parametresi YOK): bir portal kullanıcısı yalnız
//! kendi kurumunun markasını görür. Yazma/silme admin ağacındadır
//! (`routes/org_branding.rs`, X-Admin-Key).

use super::jwt::PortalActor;
use crate::branding::{self, BrandingSummary};
use crate::{error::AppError, state::AppState};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use utoipa_axum::{router::OpenApiRouter, routes};
use wf_org::repo;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(branding_summary))
        .routes(routes!(branding_asset))
        .with_state(state)
}

#[utoipa::path(get, path = "", tag = "portal",
    responses((status = 200, description = "Oturumdaki tenant'ın marka özeti", body = BrandingSummary)),
    security(("bearer_jwt" = [])))]
async fn branding_summary(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<BrandingSummary>, AppError> {
    let tenant = repo::orgtnt::get(&s.pool, actor.orgtnt_id).await?;
    Ok(Json(BrandingSummary::from(&tenant)))
}

#[utoipa::path(get, path = "/logo/{slot}", tag = "portal",
    params(("slot" = String, Path, description = "Varlık slotu: logo | favicon")),
    responses(
        (status = 200, description = "Görsel içeriği", body = Vec<u8>),
        (status = 404, description = "Slot boş"),
    ),
    security(("bearer_jwt" = [])))]
async fn branding_asset(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(slot): Path<String>,
) -> Result<(StatusCode, HeaderMap, Bytes), AppError> {
    let slot = branding::parse_slot(&slot)?;
    let tenant = repo::orgtnt::get(&s.pool, actor.orgtnt_id).await?;
    let (h, bytes) = branding::read(&s.wfd.storage, &tenant, slot).await?;
    Ok((StatusCode::OK, h, bytes))
}
