use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrgError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Kaynak durumu isteği reddediyor (409). Taşınan metin MAKİNE KODUDUR
    /// (`permission.in_use` gibi) — server katmanı bunu `AppError.code`'a çevirir,
    /// istemci hata METNİNİ parse etmez.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
