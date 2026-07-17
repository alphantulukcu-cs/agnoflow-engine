use crate::{error::WfdError, models::Project};
use sqlx::PgPool;
use uuid::Uuid;

const COLS: &str = "project_id, orgtnt_id, name, description, created_at, updated_at";

pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Project, WfdError> {
    sqlx::query_as::<_, Project>(&format!(
        "INSERT INTO wf.project (orgtnt_id, name, description) \
         VALUES ($1,$2,$3) RETURNING {COLS}"
    ))
    .bind(orgtnt_id).bind(name).bind(description)
    .fetch_one(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.constraint()) {
        Some("project_orgtnt_id_name_key") =>
            WfdError::Conflict(format!("{name}: bu isimde proje zaten var")),
        _ => WfdError::Database(e),
    })
}

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Project>, WfdError> {
    sqlx::query_as::<_, Project>(&format!(
        "SELECT {COLS} FROM wf.project WHERE orgtnt_id=$1 ORDER BY created_at"
    ))
    .bind(orgtnt_id)
    .fetch_all(pool).await
    .map_err(WfdError::Database)
}

pub async fn get(pool: &PgPool, project_id: Uuid) -> Result<Project, WfdError> {
    sqlx::query_as::<_, Project>(&format!(
        "SELECT {COLS} FROM wf.project WHERE project_id=$1"
    ))
    .bind(project_id)
    .fetch_optional(pool).await?
    .ok_or_else(|| WfdError::NotFound(format!("project {project_id}")))
}

pub async fn update(
    pool: &PgPool,
    project_id: Uuid,
    name: Option<&str>,
    description: Option<&str>,
) -> Result<Project, WfdError> {
    sqlx::query_as::<_, Project>(&format!(
        "UPDATE wf.project \
         SET name = COALESCE($2, name), \
             description = COALESCE($3, description), \
             updated_at = now() \
         WHERE project_id=$1 RETURNING {COLS}"
    ))
    .bind(project_id).bind(name).bind(description)
    .fetch_optional(pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.constraint()) {
        Some("project_orgtnt_id_name_key") =>
            WfdError::Conflict("Bu isimde başka bir proje zaten var".into()),
        _ => WfdError::Database(e),
    })?
    .ok_or_else(|| WfdError::NotFound(format!("project {project_id}")))
}

/// project_id verilmeyen eski istemciler için tenant'ın varsayılan projesi:
/// en eski proje; hiç yoksa "Test Project" yaratılır.
pub async fn resolve_default(pool: &PgPool, orgtnt_id: Uuid) -> Result<Uuid, WfdError> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM wf.project WHERE orgtnt_id=$1 ORDER BY created_at LIMIT 1"
    )
    .bind(orgtnt_id)
    .fetch_optional(pool).await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    // Eşzamanlı çağrıda unique çakışırsa mevcut satırı al (ON CONFLICT DO NOTHING → yeniden oku).
    sqlx::query(
        "INSERT INTO wf.project (orgtnt_id, name) VALUES ($1, 'Test Project') \
         ON CONFLICT (orgtnt_id, name) DO NOTHING"
    )
    .bind(orgtnt_id)
    .execute(pool).await?;
    let id: Uuid = sqlx::query_scalar(
        "SELECT project_id FROM wf.project WHERE orgtnt_id=$1 ORDER BY created_at LIMIT 1"
    )
    .bind(orgtnt_id)
    .fetch_one(pool).await?;
    Ok(id)
}

/// Projenin tenant'ına ait olduğunu doğrular (cross-tenant sızıntı kapısı).
pub async fn assert_in_tenant(
    pool: &PgPool,
    project_id: Uuid,
    orgtnt_id: Uuid,
) -> Result<(), WfdError> {
    let ok: Option<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM wf.project WHERE project_id=$1 AND orgtnt_id=$2"
    )
    .bind(project_id).bind(orgtnt_id)
    .fetch_optional(pool).await?;
    ok.map(|_| ())
        .ok_or_else(|| WfdError::NotFound(format!("project {project_id} (tenant uyuşmuyor)")))
}
