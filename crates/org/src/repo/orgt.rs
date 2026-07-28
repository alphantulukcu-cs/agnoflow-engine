use crate::{error::OrgError, models::Orgt};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn list_by_tenant(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Orgt>, OrgError> {
    sqlx::query_as::<_, Orgt>(
        "SELECT orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at
         FROM org.orgt WHERE orgtnt_id = $1 ORDER BY name LIMIT $2 OFFSET $3",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Bir org ağacının bağlı olduğu tenant — orgu create route'u orgtnt_id'yi buradan çözer.
pub async fn get_orgtnt_id(pool: &PgPool, orgt_id: Uuid) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>("SELECT orgtnt_id FROM org.orgt WHERE orgt_id = $1")
        .bind(orgt_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound(format!("orgt {orgt_id}")))
}
