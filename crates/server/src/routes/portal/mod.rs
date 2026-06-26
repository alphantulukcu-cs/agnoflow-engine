pub mod auth;
pub mod jwt;
pub mod pool;
pub mod wfd;
pub mod wfe;

use axum::Router;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/auth", auth::router(state.clone()))
        .nest("/pool", pool::router(state.clone()))
        .nest("/wfd",  wfd::router(state.clone()))
        .nest("/wfe",  wfe::router(state))
}
