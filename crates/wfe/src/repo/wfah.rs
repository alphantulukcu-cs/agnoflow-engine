use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfeError, models::WfahRow};

pub async fn append(
    pool:   &PgPool,
    wfe_id: Uuid,
    seq:    i32,
    action: &str,
    actor:  &serde_json::Value,
    input:  Option<&serde_json::Value>,
) -> Result<(), WfeError> {
    sqlx::query(
        "INSERT INTO wf.wfah (wfe_id, seq, action, actor, input)
         VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(wfe_id)
    .bind(seq)
    .bind(action)
    .bind(actor)
    .bind(input)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<WfahRow>, WfeError> {
    sqlx::query_as::<_, WfahRow>(
        "SELECT wfah_id, wfe_id, seq, action, actor, input, applied_at
         FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq ASC"
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

pub async fn next_seq(pool: &PgPool, wfe_id: Uuid) -> Result<i32, WfeError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(seq) FROM wf.wfah WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
