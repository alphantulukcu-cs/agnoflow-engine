use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgtnt};
use chrono::Utc;

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

pub async fn create(pool: &PgPool, name: String, code: String) -> Result<Orgtnt, OrgError> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query_as::<_, Orgtnt>(
        "INSERT INTO org.orgtnt (orgtnt_id, name, code, is_active, created_at, updated_at)
         VALUES ($1, $2, $3, true, $4, $5)
         RETURNING orgtnt_id, name, code, is_active, created_at, updated_at"
    )
    .bind(id)
    .bind(&name)
    .bind(&code)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(pool: &PgPool, id: Uuid, name: String, code: String) -> Result<Orgtnt, OrgError> {
    let now = Utc::now();

    sqlx::query_as::<_, Orgtnt>(
        "UPDATE org.orgtnt SET name = $1, code = $2, updated_at = $3
         WHERE orgtnt_id = $4
         RETURNING orgtnt_id, name, code, is_active, created_at, updated_at"
    )
    .bind(&name)
    .bind(&code)
    .bind(now)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), OrgError> {
    let result = sqlx::query("DELETE FROM org.orgtnt WHERE orgtnt_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(OrgError::Database)?;

    if result.rows_affected() == 0 {
        return Err(OrgError::NotFound(format!("orgtnt {id}")));
    }
    Ok(())
}
