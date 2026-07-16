use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
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

#[derive(Deserialize)]
struct ListQuery {
    orgtnt_id: Uuid,
}

async fn list_projects(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Project>>, AppError> {
    wf_wfd::project::list(&s.pool, q.orgtnt_id)
        .await
        .map(Json)
        .map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct CreateBody {
    orgtnt_id: Uuid,
    name: String,
    #[serde(default)]
    description: Option<String>,
}

async fn create_project(
    State(s): State<AppState>,
    Json(b): Json<CreateBody>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    let name = b.name.trim();
    if name.is_empty() {
        return Err(AppError("Proje adı boş olamaz".into(), StatusCode::BAD_REQUEST));
    }
    wf_wfd::project::create(&s.pool, b.orgtnt_id, name, b.description.as_deref())
        .await
        .map(|p| (StatusCode::CREATED, Json(p)))
        .map_err(map_wfd_err)
}

async fn get_project(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Project>, AppError> {
    wf_wfd::project::get(&s.pool, id)
        .await
        .map(Json)
        .map_err(map_wfd_err)
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
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateBody>,
) -> Result<Json<Project>, AppError> {
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
