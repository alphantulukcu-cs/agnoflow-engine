use thiserror::Error;
use crate::autoexec::error::AutoexecError;

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
    #[error("autoexec error: {0}")]
    AutoexecError(#[from] AutoexecError),
    #[error("unknown autoexec type: {0}")]
    UnknownAutoexecType(String),
}
