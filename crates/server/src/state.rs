use std::sync::Arc;
use sqlx::PgPool;
use wf_wfe::WfeExecutor;
use wf_wfd::WfdAdapter;

#[derive(Clone)]
pub struct AppState {
    pub pool:     PgPool,
    pub executor: Arc<WfeExecutor>,
    pub wfd:      Arc<WfdAdapter>,
    pub cfg:      Arc<crate::config::Config>,
}
