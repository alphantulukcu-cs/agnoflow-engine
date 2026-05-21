use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgt};

pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Orgt>, OrgError> {
    sqlx::query_as::<_, Orgt>(
        "SELECT orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at
         FROM org.orgt WHERE orgtnt_id = $1 ORDER BY name"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}
