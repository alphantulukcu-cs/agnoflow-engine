// Predefined schema (WFD şablonu) uçları.
// Yazma kuralı: scope='global' → yalnız tenant admin;
// scope='project' → tenant admin YA DA o projenin admin'i.
// Genel adminin şablonuna proje admini dokunamaz; proje admininkine
// tenant admin dokunabilir. Kullanıcılar yalnız seçilebilir listeyi görür.

use super::auth::{require_can_design, require_can_manage_project, AppAuth};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wf_wfd::template::{self, WfdTemplate};

fn default_kind() -> String { "workflow".into() }

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_selectable).post(create_template))
        .route("/manage", get(list_manageable))
        .route("/:id", get(get_template_meta).patch(update_template).delete(delete_template))
        .route("/:id/json", get(template_json))
        .route("/:id/usage", get(template_usage))
        .route("/:id/visibility", get(get_visibility).put(put_visibility))
        .with_state(state)
}

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_) => StatusCode::NOT_FOUND,
        E::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}

/// Şablonu YÖNETME yetkisi: global → tenant admin; project → tenant admin
/// ya da o projenin admin'i. (Proje adminleri global şablona dokunamaz.)
async fn require_can_manage_template(
    s: &AppState,
    auth: &AppAuth,
    tpl: &WfdTemplate,
) -> Result<(), AppError> {
    if tpl.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Şablon bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    if auth.role == "admin" {
        return Ok(());
    }
    match (tpl.scope.as_str(), tpl.project_id) {
        ("project", Some(pid)) => require_can_manage_project(&s.pool, auth, pid).await,
        _ => Err(AppError(
            "Global şablonları yalnız tenant admin yönetir".into(),
            StatusCode::FORBIDDEN,
        )),
    }
}

#[derive(Deserialize)]
struct SelectableQuery {
    project_id: Uuid,
    /// 'workflow' | 'context' — verilmezse ikisi de döner.
    #[serde(default)]
    kind: Option<String>,
}

/// Yeni WFD galerisinde görünecek şablonlar — kullanıcı ve proje süzgeçli.
async fn list_selectable(
    State(s): State<AppState>,
    auth: AppAuth,
    Query(q): Query<SelectableQuery>,
) -> Result<Json<Vec<WfdTemplate>>, AppError> {
    // Projede çalışma hakkı olmayan, galeriyi de göremez.
    require_can_design(&s.pool, &auth, q.project_id).await?;
    template::list_selectable(&s.pool, auth.orgtnt_id, q.project_id, auth.user_id, auth.role == "admin", q.kind.as_deref())
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct ManageQuery {
    #[serde(default)]
    kind: Option<String>,
}

async fn list_manageable(
    State(s): State<AppState>,
    auth: AppAuth,
    Query(q): Query<ManageQuery>,
) -> Result<Json<Vec<WfdTemplate>>, AppError> {
    template::list_manageable(&s.pool, auth.orgtnt_id, auth.user_id, auth.role == "admin", q.kind.as_deref())
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct CreateTemplateBody {
    /// 'workflow' (varsayılan) | 'context'
    #[serde(default = "default_kind")]
    kind: String,
    scope: String, // 'global' | 'project'
    #[serde(default)]
    project_id: Option<Uuid>,
    name: String,
    #[serde(default)]
    description: Option<String>,
    wfd: Value,
    /// Yalnız global scope: seçilebileceği projeler (boş/verilmemiş = tümü).
    #[serde(default)]
    visible_project_ids: Option<Vec<Uuid>>,
    /// Görebilecek kullanıcılar (boş/verilmemiş = herkes).
    #[serde(default)]
    visible_user_ids: Option<Vec<Uuid>>,
}

async fn create_template(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateTemplateBody>,
) -> Result<(StatusCode, Json<WfdTemplate>), AppError> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err(AppError("Şablon adı boş olamaz".into(), StatusCode::BAD_REQUEST));
    }
    match b.scope.as_str() {
        "global" => {
            auth.require_admin()?;
            if b.project_id.is_some() {
                return Err(AppError(
                    "Global şablonda project_id verilmez".into(),
                    StatusCode::BAD_REQUEST,
                ));
            }
        }
        "project" => {
            let pid = b.project_id.ok_or_else(|| {
                AppError("Proje şablonunda project_id zorunlu".into(), StatusCode::BAD_REQUEST)
            })?;
            wf_wfd::project::assert_in_tenant(&s.pool, pid, auth.orgtnt_id)
                .await
                .map_err(map_wfd_err)?;
            require_can_manage_project(&s.pool, &auth, pid).await?;
        }
        _ => {
            return Err(AppError(
                "scope 'global' ya da 'project' olmalı".into(),
                StatusCode::BAD_REQUEST,
            ))
        }
    }

    if !matches!(b.kind.as_str(), "workflow" | "context") {
        return Err(AppError("kind 'workflow' ya da 'context' olmalı".into(), StatusCode::BAD_REQUEST));
    }
    let tpl = template::create(
        &s.pool,
        auth.orgtnt_id,
        &b.kind,
        &b.scope,
        b.project_id,
        name,
        b.description.as_deref(),
        &b.wfd,
        auth.user_id,
    )
    .await
    .map_err(map_wfd_err)?;

    if b.visible_project_ids.is_some() || b.visible_user_ids.is_some() {
        // Proje kısıtı yalnız global şablonda anlamlı.
        let projects = if b.scope == "global" { b.visible_project_ids.as_deref() } else { None };
        template::set_visibility(&s.pool, tpl.template_id, projects, b.visible_user_ids.as_deref())
            .await
            .map_err(map_wfd_err)?;
    }
    Ok((StatusCode::CREATED, Json(tpl)))
}

/// Şablon metadata'sı — türetme rozeti gibi salt-okur kullanımlar için;
/// tenant'taki her oturum açmış kullanıcı okuyabilir.
async fn get_template_meta(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<WfdTemplate>, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    if tpl.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Şablon bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    Ok(Json(tpl))
}

/// Şablon AİLESİNDEN türetilmiş WFD sayısı (tüm versiyonlar üzerinden).
async fn template_usage(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    require_can_manage_template(&s, &auth, &tpl).await?;
    let derived: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM wf.wfd_meta w
         WHERE w.is_active = true AND w.source_template_id IN (
             SELECT template_id FROM wf.wfd_template t
             WHERE t.orgtnt_id = $1 AND t.kind = $2 AND t.scope = $3
               AND t.project_id IS NOT DISTINCT FROM $4 AND t.name = $5)",
    )
    .bind(tpl.orgtnt_id)
    .bind(&tpl.kind)
    .bind(&tpl.scope)
    .bind(tpl.project_id)
    .bind(&tpl.name)
    .fetch_one(&s.pool)
    .await
    .map_err(internal)?;
    Ok(Json(serde_json::json!({ "derived_wfd_count": derived })))
}

/// Şablonun ham WFD dokümanı — galeriden seçildiğinde taslağın kaynağı.
async fn template_json(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    if tpl.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Şablon bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    template::get_json(&s.pool, id).await.map(Json).map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct UpdateTemplateBody {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
}

async fn update_template(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateTemplateBody>,
) -> Result<Json<WfdTemplate>, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    require_can_manage_template(&s, &auth, &tpl).await?;
    template::update_meta(&s.pool, id, b.description.as_deref(), b.is_active)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

async fn delete_template(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    require_can_manage_template(&s, &auth, &tpl).await?;
    template::delete(&s.pool, id).await.map_err(map_wfd_err)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_visibility(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    require_can_manage_template(&s, &auth, &tpl).await?;
    let (projects, users) = template::visibility(&s.pool, id).await.map_err(map_wfd_err)?;
    Ok(Json(serde_json::json!({
        "visible_project_ids": projects,
        "visible_user_ids": users,
    })))
}

#[derive(Deserialize)]
struct VisibilityBody {
    #[serde(default)]
    visible_project_ids: Option<Vec<Uuid>>,
    #[serde(default)]
    visible_user_ids: Option<Vec<Uuid>>,
}

async fn put_visibility(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
    Json(b): Json<VisibilityBody>,
) -> Result<StatusCode, AppError> {
    let tpl = template::get(&s.pool, id).await.map_err(map_wfd_err)?;
    require_can_manage_template(&s, &auth, &tpl).await?;
    // Proje kısıtı yalnız global şablonda; verilen projeler tenant'a ait olmalı.
    let projects = if tpl.scope == "global" { b.visible_project_ids.as_deref() } else { None };
    if let Some(pids) = projects {
        for pid in pids {
            wf_wfd::project::assert_in_tenant(&s.pool, *pid, auth.orgtnt_id)
                .await
                .map_err(|_| AppError(format!("Proje bulunamadı: {pid}"), StatusCode::NOT_FOUND))?;
        }
    }
    if let Some(uids) = b.visible_user_ids.as_deref() {
        for uid in uids {
            let ok: Option<Uuid> = sqlx::query_scalar(
                "SELECT user_id FROM wf.app_user WHERE user_id = $1 AND orgtnt_id = $2",
            )
            .bind(uid)
            .bind(auth.orgtnt_id)
            .fetch_optional(&s.pool)
            .await
            .map_err(internal)?;
            if ok.is_none() {
                return Err(AppError(format!("Kullanıcı bulunamadı: {uid}"), StatusCode::NOT_FOUND));
            }
        }
    }
    template::set_visibility(&s.pool, id, projects, b.visible_user_ids.as_deref())
        .await
        .map_err(map_wfd_err)?;
    Ok(StatusCode::NO_CONTENT)
}
