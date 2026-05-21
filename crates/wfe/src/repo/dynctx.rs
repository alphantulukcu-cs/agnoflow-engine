use sqlx::PgPool;
use uuid::Uuid;
use crate::error::WfeError;

/// Insert a new DynCtx snapshot. Insert-only — never update existing rows.
pub async fn insert(
    pool:   &PgPool,
    wfe_id: Uuid,
    seq:    i32,
    ctx:    &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)"
    )
    .bind(wfe_id)
    .bind(seq)
    .bind(ctx)
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns the latest DynCtx snapshot for a WFE.
pub async fn load_latest(
    pool:   &PgPool,
    wfe_id: Uuid,
) -> Result<serde_json::Value, WfeError> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT ctx FROM wf.wfe_dynctx
         WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1"
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(format!("dynctx for wfe {wfe_id}")))
}

pub async fn next_seq(pool: &PgPool, wfe_id: Uuid) -> Result<i32, WfeError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(seq) FROM wf.wfe_dynctx WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
