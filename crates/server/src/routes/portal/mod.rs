pub mod auth;
pub mod jwt;
pub mod pool;

use axum::Router;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/auth", auth::router(state.clone()))
        .nest("/pool", pool::router(state))
}
