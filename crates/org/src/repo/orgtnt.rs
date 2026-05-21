use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgtnt};

pub async fn list(pool: &PgPool) -> Result<Vec<Orgtnt>, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "SELECT orgtnt_id, name, code, is_active, created_at, updated_at
         FROM org.orgtnt ORDER BY name"
    )
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Orgtnt, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "SELECT orgtnt_id, name, code, is_active, created_at, updated_at
         FROM org.orgtnt WHERE orgtnt_id = $1"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}
