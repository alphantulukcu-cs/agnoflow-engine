// Predefined schema (WFD şablonu) deposu. Her versiyon immutable snapshot;
// aynı (scope, proje, ad) için create çağrısı versiyonu otomatik artırır.

use crate::error::WfdError;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct WfdTemplate {
    pub template_id: Uuid,
    pub orgtnt_id:   Uuid,
    /// 'workflow' → WFD dokümanı; 'context' → {properties, required} şeması.
    pub kind:        String,
    pub scope:       String,
    pub project_id:  Option<Uuid>,
    pub name:        String,
    pub description: Option<String>,
    pub version:     i32,
    pub created_by:  Uuid,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

const COLS: &str = "template_id, orgtnt_id, kind, scope, project_id, name, description, \
                    version, created_by, is_active, created_at, updated_at";

/// Yeni şablon (ya da aynı ada yeni versiyon) ekler.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    kind: &str,
    scope: &str,
    project_id: Option<Uuid>,
    name: &str,
    description: Option<&str>,
    wfd_json: &Value,
    created_by: Uuid,
) -> Result<WfdTemplate, WfdError> {
    let version: i32 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM wf.wfd_template \
         WHERE orgtnt_id = $1 AND kind = $2 AND scope = $3 \
           AND project_id IS NOT DISTINCT FROM $4 AND name = $5",
    )
    .bind(orgtnt_id)
    .bind(kind)
    .bind(scope)
    .bind(project_id)
    .bind(name)
    .fetch_one(pool)
    .await?;

    let tpl = sqlx::query_as::<_, WfdTemplate>(&format!(
        "INSERT INTO wf.wfd_template \
         (orgtnt_id, kind, scope, project_id, name, description, version, wfd_json, created_by) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING {COLS}",
    ))
    .bind(orgtnt_id)
    .bind(kind)
    .bind(scope)
    .bind(project_id)
    .bind(name)
    .bind(description)
    .bind(version)
    .bind(wfd_json)
    .bind(created_by)
    .fetch_one(pool)
    .await
    .map_err(WfdError::Database)?;

    // Görünürlük AİLE düzeyindedir: yeni versiyon bir önceki versiyonun
    // proje/kullanıcı kısıtlarını devralır (kısıtsız aile kısıtsız kalır).
    if version > 1 {
        sqlx::query(
            "INSERT INTO wf.wfd_template_project (template_id, project_id)
             SELECT $1, tp.project_id FROM wf.wfd_template_project tp
             JOIN wf.wfd_template t ON t.template_id = tp.template_id
             WHERE t.orgtnt_id = $2 AND t.kind = $7 AND t.scope = $3
               AND t.project_id IS NOT DISTINCT FROM $4 AND t.name = $5 AND t.version = $6",
        )
        .bind(tpl.template_id)
        .bind(orgtnt_id)
        .bind(scope)
        .bind(project_id)
        .bind(name)
        .bind(version - 1)
        .bind(kind)
        .execute(pool)
        .await?;
        sqlx::query(
            "INSERT INTO wf.wfd_template_user (template_id, user_id)
             SELECT $1, tu.user_id FROM wf.wfd_template_user tu
             JOIN wf.wfd_template t ON t.template_id = tu.template_id
             WHERE t.orgtnt_id = $2 AND t.kind = $7 AND t.scope = $3
               AND t.project_id IS NOT DISTINCT FROM $4 AND t.name = $5 AND t.version = $6",
        )
        .bind(tpl.template_id)
        .bind(orgtnt_id)
        .bind(scope)
        .bind(project_id)
        .bind(name)
        .bind(version - 1)
        .bind(kind)
        .execute(pool)
        .await?;
    }
    Ok(tpl)
}

/// Aynı ailenin (tenant, scope, proje, ad) tüm versiyon id'leri.
async fn family_ids(pool: &PgPool, tpl: &WfdTemplate) -> Result<Vec<Uuid>, WfdError> {
    sqlx::query_scalar(
        "SELECT template_id FROM wf.wfd_template
         WHERE orgtnt_id = $1 AND kind = $5 AND scope = $2
           AND project_id IS NOT DISTINCT FROM $3 AND name = $4",
    )
    .bind(tpl.orgtnt_id)
    .bind(&tpl.scope)
    .bind(tpl.project_id)
    .bind(&tpl.name)
    .bind(&tpl.kind)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

pub async fn get(pool: &PgPool, template_id: Uuid) -> Result<WfdTemplate, WfdError> {
    sqlx::query_as::<_, WfdTemplate>(&format!(
        "SELECT {COLS} FROM wf.wfd_template WHERE template_id = $1",
    ))
    .bind(template_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("template {template_id}")))
}

pub async fn get_json(pool: &PgPool, template_id: Uuid) -> Result<Value, WfdError> {
    sqlx::query_scalar("SELECT wfd_json FROM wf.wfd_template WHERE template_id = $1")
        .bind(template_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| WfdError::NotFound(format!("template {template_id}")))
}

/// Bir kullanıcının X projesinde yeni WFD açarken SEÇEBİLECEĞİ şablonlar:
/// her ad için en yüksek aktif versiyon; global'de proje kısıtı (boş = tüm
/// projeler) ve kullanıcı kısıtı (boş = herkes) uygulanır. Tenant admin'de
/// kullanıcı kısıtı atlanır (`is_admin`).
pub async fn list_selectable(
    pool: &PgPool,
    orgtnt_id: Uuid,
    project_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    kind: Option<&str>,
) -> Result<Vec<WfdTemplate>, WfdError> {
    sqlx::query_as::<_, WfdTemplate>(&format!(
        "SELECT DISTINCT ON (kind, scope, project_id, name) {COLS}
         FROM wf.wfd_template t
         WHERE t.orgtnt_id = $1 AND t.is_active = true
           AND (
             (t.scope = 'project' AND t.project_id = $2)
             OR (t.scope = 'global' AND (
                   NOT EXISTS (SELECT 1 FROM wf.wfd_template_project tp WHERE tp.template_id = t.template_id)
                   OR EXISTS (SELECT 1 FROM wf.wfd_template_project tp
                              WHERE tp.template_id = t.template_id AND tp.project_id = $2)
             ))
           )
           AND ($4 OR
             NOT EXISTS (SELECT 1 FROM wf.wfd_template_user tu WHERE tu.template_id = t.template_id)
             OR EXISTS (SELECT 1 FROM wf.wfd_template_user tu
                        WHERE tu.template_id = t.template_id AND tu.user_id = $3)
           )
           AND ($5::text IS NULL OR t.kind = $5)
         ORDER BY kind, scope, project_id, name, version DESC",
    ))
    .bind(orgtnt_id)
    .bind(project_id)
    .bind(user_id)
    .bind(is_admin)
    .bind(kind)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

/// Yönetim listesi: tenant admin tümünü, proje admini yalnız admin olduğu
/// projelerin 'project' scope şablonlarını görür (tüm versiyonlar).
pub async fn list_manageable(
    pool: &PgPool,
    orgtnt_id: Uuid,
    user_id: Uuid,
    is_admin: bool,
    kind: Option<&str>,
) -> Result<Vec<WfdTemplate>, WfdError> {
    sqlx::query_as::<_, WfdTemplate>(&format!(
        "SELECT {COLS} FROM wf.wfd_template t
         WHERE t.orgtnt_id = $1
           AND ($3 OR (t.scope = 'project' AND EXISTS (
                 SELECT 1 FROM wf.project_member m
                 WHERE m.project_id = t.project_id AND m.user_id = $2 AND m.role = 'admin')))
           AND ($4::text IS NULL OR t.kind = $4)
         ORDER BY name, version DESC",
    ))
    .bind(orgtnt_id)
    .bind(user_id)
    .bind(is_admin)
    .bind(kind)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

/// Meta güncelleme: açıklama / aktiflik. (Ad ve içerik yeni versiyonla değişir.)
pub async fn update_meta(
    pool: &PgPool,
    template_id: Uuid,
    description: Option<&str>,
    is_active: Option<bool>,
) -> Result<WfdTemplate, WfdError> {
    sqlx::query_as::<_, WfdTemplate>(&format!(
        "UPDATE wf.wfd_template SET \
             description = COALESCE($2, description), \
             is_active = COALESCE($3, is_active), \
             updated_at = now() \
         WHERE template_id = $1 RETURNING {COLS}",
    ))
    .bind(template_id)
    .bind(description)
    .bind(is_active)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("template {template_id}")))
}

pub async fn delete(pool: &PgPool, template_id: Uuid) -> Result<(), WfdError> {
    let n = sqlx::query("DELETE FROM wf.wfd_template WHERE template_id = $1")
        .bind(template_id)
        .execute(pool)
        .await?
        .rows_affected();
    if n == 0 {
        return Err(WfdError::NotFound(format!("template {template_id}")));
    }
    Ok(())
}

/// Görünürlük atamalarını değiştirir (replace semantiği) — kısıt AİLENİN
/// TÜM versiyonlarına uygulanır; tek versiyona kısıt diye bir şey yoktur.
/// `visible_projects`: yalnız global scope için anlamlı; None = dokunma.
/// `visible_users`: None = dokunma; Some(boş) = herkes.
pub async fn set_visibility(
    pool: &PgPool,
    template_id: Uuid,
    visible_projects: Option<&[Uuid]>,
    visible_users: Option<&[Uuid]>,
) -> Result<(), WfdError> {
    let tpl = get(pool, template_id).await?;
    let ids = family_ids(pool, &tpl).await?;
    let mut tx = pool.begin().await?;
    if let Some(projects) = visible_projects {
        sqlx::query("DELETE FROM wf.wfd_template_project WHERE template_id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        for id in &ids {
            for pid in projects {
                sqlx::query(
                    "INSERT INTO wf.wfd_template_project (template_id, project_id) VALUES ($1,$2)",
                )
                .bind(id)
                .bind(pid)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    if let Some(users) = visible_users {
        sqlx::query("DELETE FROM wf.wfd_template_user WHERE template_id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        for id in &ids {
            for uid in users {
                sqlx::query(
                    "INSERT INTO wf.wfd_template_user (template_id, user_id) VALUES ($1,$2)",
                )
                .bind(id)
                .bind(uid)
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Şablonun görünürlük atamaları (yönetim UI'ı için).
pub async fn visibility(
    pool: &PgPool,
    template_id: Uuid,
) -> Result<(Vec<Uuid>, Vec<Uuid>), WfdError> {
    let projects: Vec<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM wf.wfd_template_project WHERE template_id = $1",
    )
    .bind(template_id)
    .fetch_all(pool)
    .await?;
    let users: Vec<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM wf.wfd_template_user WHERE template_id = $1",
    )
    .bind(template_id)
    .fetch_all(pool)
    .await?;
    Ok((projects, users))
}
