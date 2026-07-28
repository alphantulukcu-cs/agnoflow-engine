use crate::{error::OrgError, models::Orgt};
use sqlx::PgPool;
use uuid::Uuid;

const SEL: &str = "orgt_id, orgtnt_id, name, description, is_active, is_default, created_at, updated_at";

pub async fn list_by_tenant(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Orgt>, OrgError> {
    sqlx::query_as::<_, Orgt>(&format!(
        "SELECT {SEL} FROM org.orgt WHERE orgtnt_id = $1 ORDER BY name LIMIT $2 OFFSET $3"
    ))
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

/// Tenant'ın hiç aktif ağacı yoksa yeni ağaç otomatik varsayılan olur — "bir tanesi
/// default olacak" hiçbir zaman ihlal edilmez.
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Orgt, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("ağaç adı boş olamaz".into()));
    }
    let has_active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org.orgt WHERE orgtnt_id = $1 AND is_active = true)",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await?;

    sqlx::query_as::<_, Orgt>(&format!(
        "INSERT INTO org.orgt (orgtnt_id, name, description, is_default)
         VALUES ($1, $2, $3, $4)
         RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(name)
    .bind(description)
    .bind(!has_active)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(
    pool: &PgPool,
    orgt_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Orgt, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("ağaç adı boş olamaz".into()));
    }
    sqlx::query_as::<_, Orgt>(&format!(
        "UPDATE org.orgt SET name = $2, description = $3, updated_at = now()
         WHERE orgt_id = $1
         RETURNING {SEL}"
    ))
    .bind(orgt_id)
    .bind(name)
    .bind(description)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgt {orgt_id}")))
}

/// Transaction: önce tenant'ın mevcut varsayılanı false'a düşer, sonra hedef true olur —
/// partial unique index (`orgt_one_default_per_tenant`) hiçbir ara adımda ihlal edilmez.
pub async fn set_default(pool: &PgPool, orgtnt_id: Uuid, orgt_id: Uuid) -> Result<Orgt, OrgError> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE org.orgt SET is_default = false WHERE orgtnt_id = $1 AND is_default = true")
        .bind(orgtnt_id)
        .execute(&mut *tx)
        .await?;

    let result = sqlx::query("UPDATE org.orgt SET is_default = true WHERE orgt_id = $1 AND orgtnt_id = $2")
        .bind(orgt_id)
        .bind(orgtnt_id)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        return Err(OrgError::NotFound(format!("orgt {orgt_id} bu tenant'ta yok")));
    }

    tx.commit().await?;

    sqlx::query_as::<_, Orgt>(&format!("SELECT {SEL} FROM org.orgt WHERE orgt_id = $1"))
        .bind(orgt_id)
        .fetch_one(pool)
        .await
        .map_err(OrgError::Database)
}
