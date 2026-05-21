use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfeError, models::WfeRow};

pub async fn create(
    pool:        &PgPool,
    orgtnt_id:   Uuid,
    wfd_id:      Uuid,
    wfd_version: i32,
    current_c_a: &serde_json::Value,
) -> Result<Uuid, WfeError> {
    let wfe_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfe (orgtnt_id, wfd_id, wfd_version, status, current_c_a)
         VALUES ($1, $2, $3, 'active', $4)
         RETURNING wfe_id"
    )
    .bind(orgtnt_id)
    .bind(wfd_id)
    .bind(wfd_version)
    .bind(current_c_a)
    .fetch_one(pool)
    .await?;
    Ok(wfe_id)
}

pub async fn get(pool: &PgPool, wfe_id: Uuid) -> Result<WfeRow, WfeError> {
    sqlx::query_as::<_, WfeRow>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, status,
                current_c_a, end_response, created_at, updated_at
         FROM wf.wfe WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(wfe_id.to_string()))
}

pub async fn update_c_a(
    pool:   &PgPool,
    wfe_id: Uuid,
    c_a:    &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "UPDATE wf.wfe SET current_c_a = $1, updated_at = now() WHERE wfe_id = $2"
    )
    .bind(c_a)
    .bind(wfe_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_terminal(
    pool:         &PgPool,
    wfe_id:       Uuid,
    end_response: &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "UPDATE wf.wfe
         SET status = 'terminal', end_response = $1, updated_at = now()
         WHERE wfe_id = $2"
    )
    .bind(end_response)
    .bind(wfe_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<WfeRow>, WfeError> {
    sqlx::query_as::<_, WfeRow>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, status,
                current_c_a, end_response, created_at, updated_at
         FROM wf.wfe WHERE orgtnt_id = $1 ORDER BY created_at DESC"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
