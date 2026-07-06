use thiserror::Error;

#[derive(Debug, Error)]
pub enum WfeError {
    #[error("wfe not found: {0}")]
    NotFound(String),
    #[error("wfe is terminal")]
    Terminal,
    #[error(transparent)]
    Engine(#[from] wfe_core::EngineError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
