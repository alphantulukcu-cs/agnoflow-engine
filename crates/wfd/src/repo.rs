use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfdError, models::WfdMeta};

const COLS: &str = "wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at, \
                    status, description, tags, owner, updated_at";

/// Yeni satır ekler (published veya draft). status/description/tags/owner verilir.
#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool:        &PgPool,
    wfd_id:      Uuid,
    orgtnt_id:   Uuid,
    name:        &str,
    version:     i32,
    s3_key:      &str,
    status:      &str,
    description: Option<&str>,
    tags:        &[String],
    owner:       &str,
) -> Result<Uuid, WfdError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfd_meta \
         (wfd_id, orgtnt_id, name, version, s3_key, status, description, tags, owner) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING wfd_id"
    )
    .bind(wfd_id).bind(orgtnt_id).bind(name).bind(version).bind(s3_key)
    .bind(status).bind(description).bind(tags).bind(owner)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.code()) {
        // 23505 = unique_violation → tek-draft index ihlali
        Some(code) if code == "23505" =>
            WfdError::Conflict(format!("{name}: açık draft zaten var")),
        _ => WfdError::Database(e),
    })?;
    Ok(id)
}

/// Yalnızca published (is_active) satırı döner — mevcut çalıştırma yolu.
pub async fn get_meta(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta \
                  WHERE wfd_id=$1 AND version=$2 AND is_active=true")
    )
    .bind(wfd_id).bind(version)
    .fetch_optional(pool).await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Draft dahil herhangi bir satırı döner (is_active filtresi yok).
pub async fn get_meta_any(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2")
    )
    .bind(wfd_id).bind(version)
    .fetch_optional(pool).await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

/// Liste — draft ve published birlikte döner (UI ayırır).
pub async fn list(pool: &PgPool, orgtnt_id: Uuid, limit: i64, offset: i64)
    -> Result<Vec<WfdMeta>, WfdError>
{
    sqlx::query_as::<_, WfdMeta>(
        &format!("SELECT {COLS} FROM wf.wfd_meta \
                  WHERE orgtnt_id=$1 AND is_active=true \
                  ORDER BY name, version DESC LIMIT $2 OFFSET $3")
    )
    .bind(orgtnt_id).bind(limit).bind(offset)
    .fetch_all(pool).await
    .map_err(WfdError::Database)
}

pub async fn next_version(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<i32, WfdError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(version) FROM wf.wfd_meta WHERE orgtnt_id=$1 AND name=$2"
    )
    .bind(orgtnt_id).bind(name)
    .fetch_one(pool).await?;
    Ok(max.unwrap_or(0) + 1)
}

/// Draft metadata günceller (JSON storage'da; burada sadece meta + updated_at).
pub async fn update_draft(
    pool: &PgPool, wfd_id: Uuid, version: i32,
    description: Option<&str>, tags: &[String],
) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET description=$3, tags=$4, updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version).bind(description).bind(tags)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}

/// Draft'ı published yapar (publish sonrası). status flip + updated_at.
pub async fn set_published(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let n = sqlx::query(
        "UPDATE wf.wfd_meta SET status='published', updated_at=now() \
         WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}

/// Draft satırını siler (published silinemez).
pub async fn delete_draft(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<(), WfdError> {
    let n = sqlx::query(
        "DELETE FROM wf.wfd_meta WHERE wfd_id=$1 AND version=$2 AND status='draft'"
    )
    .bind(wfd_id).bind(version)
    .execute(pool).await?.rows_affected();
    if n == 0 { return Err(WfdError::NotFound(format!("draft {wfd_id} v{version}"))); }
    Ok(())
}
