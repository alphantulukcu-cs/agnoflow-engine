//! Portal permission endpoint'i — JWT ağacı, SALT OKUMA, yalnız KENDİ kümesi.
//!
//! Kullanıcı id parametresi YOK: aktör token'dan çözülür, başkasının kümesi portal
//! ağacından okunamaz. Yönetim `routes/permissions.rs` (X-Admin-Key), dış uygulama
//! `routes/ext_permissions.rs` (X-Api-Key).
//!
//! Küme JWT'ye GÖMÜLMEZ, her istekte DB'den okunur: token TTL'i saatlercedir —
//! gömülse bugün alınan yetki yarına kadar taşınır ve ıskarta koymanın hiçbir
//! etkisi olmazdı.

use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{extract::State, Json};
use utoipa_axum::{router::OpenApiRouter, routes};
use wf_org::{permission::EffectivePermission, repo};

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(my_permissions))
        .with_state(state)
}

#[utoipa::path(get, path = "", tag = "portal",
    responses((status = 200, description = "Oturumdaki kullanıcının etkin yetkileri + via_roles",
        body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn my_permissions(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<EffectivePermission>>, AppError> {
    repo::permission::effective_for_user(&s.pool, actor.orgtnt_id, actor.user_id)
        .await
        .map(Json)
        .map_err(Into::into)
}
