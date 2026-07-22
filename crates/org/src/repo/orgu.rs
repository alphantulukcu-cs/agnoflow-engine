use crate::{error::OrgError, models::Orgu};
use sqlx::PgPool;
use uuid::Uuid;

const SEL: &str = "o.orgu_id, oo.orgt_id, oo.orgtnt_id, oo.parent_orgu_id,
     oo.path::text AS path, o.orgu_type, o.name, o.metadata,
     (o.is_active AND oo.is_active) AS is_active,
     o.created_at, o.updated_at";

pub async fn list_by_tree(
    pool: &PgPool,
    orgt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Orgu>, OrgError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "SELECT {SEL}
         FROM org.orgu o
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
         WHERE oo.orgt_id = $1 AND o.is_active = true AND oo.is_active = true
         ORDER BY oo.path LIMIT $2 OFFSET $3"
    ))
    .bind(orgt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, orgu_id: Uuid) -> Result<Orgu, OrgError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "SELECT {SEL}
         FROM org.orgu o
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
         WHERE o.orgu_id = $1 AND o.is_active = true AND oo.is_active = true
         LIMIT 1"
    ))
    .bind(orgu_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))
}

/// Returns the orgt_id for an orgu — needed by the traversal executor.
pub async fn get_orgt_id(pool: &PgPool, orgu_id: Uuid) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>("SELECT orgt_id FROM org.orgt_orgu WHERE orgu_id = $1 LIMIT 1")
        .bind(orgu_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))
}

/// Anchor'ın tenant'ı — global tip selektörü (*:[...]) tenant genelinde çözülür.
pub async fn get_orgtnt_id(pool: &PgPool, orgu_id: Uuid) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>("SELECT orgtnt_id FROM org.orgt_orgu WHERE orgu_id = $1 LIMIT 1")
        .bind(orgu_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))
}
