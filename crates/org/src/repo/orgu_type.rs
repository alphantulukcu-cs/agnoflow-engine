use crate::{error::OrgError, models::OrguTypeDef};
use sqlx::PgPool;
use uuid::Uuid;

const SEL: &str = "type_id, orgtnt_id, key, display_name, is_active, created_at, updated_at";

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<OrguTypeDef>, OrgError> {
    sqlx::query_as::<_, OrguTypeDef>(&format!(
        "SELECT {SEL} FROM org.orgu_type_def
         WHERE orgtnt_id = $1 AND is_active = true ORDER BY display_name"
    ))
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Var olan (aktif veya pasif) aynı key'i yeniden aktifleştirir/günceller — create_role ile aynı desen.
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    key: &str,
    display_name: &str,
) -> Result<OrguTypeDef, OrgError> {
    let key = key.trim();
    let display_name = display_name.trim();
    if key.is_empty() || display_name.is_empty() {
        return Err(OrgError::BadRequest("tip anahtarı ve görünen ad boş olamaz".into()));
    }
    sqlx::query_as::<_, OrguTypeDef>(&format!(
        "INSERT INTO org.orgu_type_def (orgtnt_id, key, display_name)
         VALUES ($1, $2, $3)
         ON CONFLICT (orgtnt_id, key) DO UPDATE
         SET display_name = EXCLUDED.display_name, is_active = true, updated_at = now()
         RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(key)
    .bind(display_name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(
    pool: &PgPool,
    orgtnt_id: Uuid,
    type_id: Uuid,
    key: &str,
    display_name: &str,
) -> Result<OrguTypeDef, OrgError> {
    let key = key.trim();
    let display_name = display_name.trim();
    if key.is_empty() || display_name.is_empty() {
        return Err(OrgError::BadRequest("tip anahtarı ve görünen ad boş olamaz".into()));
    }
    sqlx::query_as::<_, OrguTypeDef>(&format!(
        "UPDATE org.orgu_type_def
         SET key = $3, display_name = $4, is_active = true, updated_at = now()
         WHERE orgtnt_id = $1 AND type_id = $2
         RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(type_id)
    .bind(key)
    .bind(display_name)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgu_type {type_id}")))
}

pub async fn deactivate(pool: &PgPool, orgtnt_id: Uuid, type_id: Uuid) -> Result<bool, OrgError> {
    let result = sqlx::query(
        "UPDATE org.orgu_type_def SET is_active = false, updated_at = now()
         WHERE orgtnt_id = $1 AND type_id = $2 AND is_active = true",
    )
    .bind(orgtnt_id)
    .bind(type_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// `key`'in bu tenant'ta aktif bir katalog kaydı olduğunu doğrular — orgu create/update
/// bunu kullanır (whitelist; katalogda yoksa `OrgError::NotFound`).
pub async fn require_active(pool: &PgPool, orgtnt_id: Uuid, key: &str) -> Result<(), OrgError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org.orgu_type_def WHERE orgtnt_id = $1 AND key = $2 AND is_active = true)",
    )
    .bind(orgtnt_id)
    .bind(key)
    .fetch_one(pool)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(OrgError::NotFound(format!("orgu tipi '{key}' katalogda yok")))
    }
}
