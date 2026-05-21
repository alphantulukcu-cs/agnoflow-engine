use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfdError, models::WfdMeta};

pub async fn insert(
    pool:      &PgPool,
    orgtnt_id: Uuid,
    name:      &str,
    version:   i32,
    s3_key:    &str,
) -> Result<Uuid, WfdError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfd_meta (orgtnt_id, name, version, s3_key)
         VALUES ($1, $2, $3, $4)
         RETURNING wfd_id"
    )
    .bind(orgtnt_id)
    .bind(name)
    .bind(version)
    .bind(s3_key)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn get_meta(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        "SELECT wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at
         FROM wf.wfd_meta
         WHERE wfd_id = $1 AND version = $2 AND is_active = true"
    )
    .bind(wfd_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<WfdMeta>, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        "SELECT wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at
         FROM wf.wfd_meta
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY name, version DESC"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

pub async fn next_version(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<i32, WfdError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(version) FROM wf.wfd_meta WHERE orgtnt_id = $1 AND name = $2"
    )
    .bind(orgtnt_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
