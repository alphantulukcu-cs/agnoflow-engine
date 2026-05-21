use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrgError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
