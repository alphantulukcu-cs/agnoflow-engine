// Proje uçları — tamamı auth ister.
// list: tenant admin hepsini, üye yalnız atandıklarını görür.
// create: yalnız tenant admin. update: tenant admin ya da o projenin admin'i.

use super::auth::{require_can_design, require_can_manage_project, AppAuth};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;
use wf_wfd::models::Project;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_projects).post(create_project))
        .route("/:id", get(get_project))
        .route("/:id", patch(update_project))
        .with_state(state)
}

async fn list_projects(
    State(s): State<AppState>,
    auth: AppAuth,
) -> Result<Json<Vec<Project>>, AppError> {
    let all = wf_wfd::project::list(&s.pool, auth.orgtnt_id)
        .await
        .map_err(map_wfd_err)?;
    if auth.role == "admin" {
        return Ok(Json(all));
    }
    let member_of: Vec<Uuid> = sqlx::query_scalar(
        "SELECT project_id FROM wf.project_member WHERE user_id = $1",
    )
    .bind(auth.user_id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(all.into_iter().filter(|p| member_of.contains(&p.project_id)).collect()))
}

#[derive(Deserialize)]
struct CreateBody {
    /// Geriye uyum için kabul edilir ama token'daki tenant esas alınır.
    #[serde(default)]
    orgtnt_id: Option<Uuid>,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateBody>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    auth.require_admin()?;
    if let Some(body_tenant) = b.orgtnt_id {
        if body_tenant != auth.orgtnt_id {
            return Err(AppError("Tenant uyuşmuyor".into(), StatusCode::FORBIDDEN));
        }
    }
    let name = b.name.trim();
    if name.is_empty() {
        return Err(AppError("Proje adı boş olamaz".into(), StatusCode::BAD_REQUEST));
    }
    wf_wfd::project::create(&s.pool, auth.orgtnt_id, name, b.description.as_deref())
        .await
        .map(|p| (StatusCode::CREATED, Json(p)))
        .map_err(map_wfd_err)
}

async fn get_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, AppError> {
    let project = wf_wfd::project::get(&s.pool, id).await.map_err(map_wfd_err)?;
    if project.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Proje bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_design(&s.pool, &auth, id).await?;
    Ok(Json(project))
}

#[derive(Deserialize)]
struct UpdateBody {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn update_project(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateBody>,
) -> Result<Json<Project>, AppError> {
    let project = wf_wfd::project::get(&s.pool, id).await.map_err(map_wfd_err)?;
    if project.orgtnt_id != auth.orgtnt_id {
        return Err(AppError("Proje bulunamadı".into(), StatusCode::NOT_FOUND));
    }
    require_can_manage_project(&s.pool, &auth, id).await?;
    let name = b.name.as_deref().map(str::trim).filter(|v| !v.is_empty());
    if b.name.is_some() && name.is_none() {
        return Err(AppError("Proje adı boş olamaz".into(), StatusCode::BAD_REQUEST));
    }
    wf_wfd::project::update(&s.pool, id, name, b.description.as_deref())
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_) => StatusCode::NOT_FOUND,
        E::Conflict(_) => StatusCode::CONFLICT,
        E::InvalidJson(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}
