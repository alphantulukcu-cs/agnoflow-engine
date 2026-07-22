use sqlx::PgPool;
use std::sync::Arc;
use wf_wfd::WfdAdapter;
use wf_wfe::WfeExecutor;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub executor: Arc<WfeExecutor>,
    pub wfd: Arc<WfdAdapter>,
    /// Ek-belge (attachment) depolaması — portal katmanı opendal store'u.
    pub attachments: Arc<crate::attachments::AttachmentStore>,
    pub cfg: Arc<crate::config::Config>,
}
