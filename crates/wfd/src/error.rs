use thiserror::Error;

#[derive(Debug, Error)]
pub enum WfdError {
    #[error("wfd not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("invalid wfd json: {0}")]
    InvalidJson(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<WfdError> for wfe_core::EngineError {
    fn from(e: WfdError) -> Self {
        wfe_core::EngineError::WfdPort(e.to_string())
    }
}
