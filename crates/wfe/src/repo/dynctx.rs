use crate::error::WfeError;
use sqlx::PgPool;
use uuid::Uuid;

/// Returns the latest DynCtx snapshot for a WFE.
pub async fn load_latest(pool: &PgPool, wfe_id: Uuid) -> Result<serde_json::Value, WfeError> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT ctx FROM wf.wfe_dynctx
         WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(format!("dynctx for wfe {wfe_id}")))
}
