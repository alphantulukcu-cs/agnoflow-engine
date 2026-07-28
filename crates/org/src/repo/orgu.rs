use crate::{error::OrgError, models::Orgu};
use sqlx::PgPool;
use uuid::Uuid;

const SEL: &str = "o.orgu_id, oo.orgt_id, oo.orgtnt_id, oo.parent_orgu_id,
     oo.path::text AS path, o.orgu_type, o.name, o.metadata,
     (o.is_active AND oo.is_active) AS is_active,
     o.created_at, o.updated_at";

/// UUID'den ltree-güvenli, deterministik path segmenti üretir. Kullanıcı adından
/// slugify ETMEZ — `metadata->>'code'` global-unique index'iyle çakışma riski taşımaz
/// ve isim değişince path'in bozulmasını önler (path immutable kalır).
pub fn orgu_path_segment(orgu_id: Uuid) -> String {
    format!("u_{}", orgu_id.simple())
}

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

pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    orgt_id: Uuid,
    parent_orgu_id: Option<Uuid>,
    name: &str,
    type_key: &str,
) -> Result<Orgu, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("birim adı boş olamaz".into()));
    }

    let orgu_id = Uuid::new_v4();
    let segment = orgu_path_segment(orgu_id);
    let orgu_type = serde_json::json!({ "type": type_key });

    let mut tx = pool.begin().await?;

    let path = match parent_orgu_id {
        None => segment,
        Some(pid) => {
            let parent_path: Option<String> = sqlx::query_scalar(
                "SELECT path::text FROM org.orgt_orgu
                 WHERE orgu_id = $1 AND orgt_id = $2 AND is_active = true",
            )
            .bind(pid)
            .bind(orgt_id)
            .fetch_optional(&mut *tx)
            .await?;
            let parent_path = parent_path
                .ok_or_else(|| OrgError::NotFound(format!("ebeveyn orgu {pid} bu ağaçta yok")))?;
            format!("{parent_path}.{segment}")
        }
    };

    sqlx::query("INSERT INTO org.orgu (orgu_id, orgu_type, name) VALUES ($1, $2, $3)")
        .bind(orgu_id)
        .bind(&orgu_type)
        .bind(name)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO org.orgt_orgu (orgt_id, orgu_id, orgtnt_id, parent_orgu_id, path)
         VALUES ($1, $2, $3, $4, $5::ltree)",
    )
    .bind(orgt_id)
    .bind(orgu_id)
    .bind(orgtnt_id)
    .bind(parent_orgu_id)
    .bind(&path)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    get(pool, orgu_id).await
}

pub async fn update(pool: &PgPool, orgu_id: Uuid, name: &str, type_key: &str) -> Result<Orgu, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("birim adı boş olamaz".into()));
    }
    let orgu_type = serde_json::json!({ "type": type_key });
    let result = sqlx::query(
        "UPDATE org.orgu SET name = $2, orgu_type = $3, updated_at = now()
         WHERE orgu_id = $1 AND is_active = true",
    )
    .bind(orgu_id)
    .bind(name)
    .bind(&orgu_type)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(OrgError::NotFound(format!("orgu {orgu_id}")));
    }
    get(pool, orgu_id).await
}

/// Hedef birimi ve TÜM alt ağacını pasifleştirir (soft-delete, cascade). `org.orgu`
/// satırı yalnızca başka aktif `orgt_orgu` konumu kalmadıysa pasifleştirilir (şema
/// çoklu-ağaç konumlandırmaya izin veriyor; pratikte tek konum olsa da savunmacı).
/// Dönüş: pasifleştirilen birim sayısı (hedef dahil).
pub async fn delete_cascade(pool: &PgPool, orgu_id: Uuid) -> Result<i64, OrgError> {
    let target: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT orgt_id, path::text FROM org.orgt_orgu WHERE orgu_id = $1 AND is_active = true LIMIT 1",
    )
    .bind(orgu_id)
    .fetch_optional(pool)
    .await?;
    let (orgt_id, path) = target.ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))?;

    let mut tx = pool.begin().await?;

    let affected: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE org.orgt_orgu SET is_active = false, updated_at = now()
         WHERE orgt_id = $1 AND path <@ $2::ltree AND is_active = true
         RETURNING orgu_id",
    )
    .bind(orgt_id)
    .bind(&path)
    .fetch_all(&mut *tx)
    .await?;

    if !affected.is_empty() {
        sqlx::query(
            "UPDATE org.orgu SET is_active = false, updated_at = now()
             WHERE orgu_id = ANY($1)
               AND NOT EXISTS (
                   SELECT 1 FROM org.orgt_orgu oo
                   WHERE oo.orgu_id = org.orgu.orgu_id AND oo.is_active = true
               )",
        )
        .bind(&affected)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(affected.len() as i64)
}

#[cfg(test)]
mod tests {
    use super::orgu_path_segment;
    use uuid::Uuid;

    #[test]
    fn path_segment_is_ltree_safe_and_stable() {
        let id = Uuid::parse_str("a1b2c3d4-0000-1111-2222-333344445555").unwrap();
        let segment = orgu_path_segment(id);
        assert_eq!(segment, format!("u_{}", id.simple()));
        assert!(segment.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        // İki çağrı aynı sonucu verir (deterministik — path üretimi idempotent).
        assert_eq!(segment, orgu_path_segment(id));
    }
}
