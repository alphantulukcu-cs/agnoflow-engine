pub mod attachments;
pub mod auth;
pub mod branding;
pub mod jwt;
pub mod notes;
pub mod pool;
pub mod wfd;
pub mod wfe;

use utoipa_axum::router::OpenApiRouter;
use crate::state::AppState;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .nest("/auth", auth::router(state.clone()))
        .nest("/branding", branding::router(state.clone()))
        .nest("/pool", pool::router(state.clone()))
        .nest("/wfd", wfd::router(state.clone()))
        .nest("/wfe", wfe::router(state))
}
