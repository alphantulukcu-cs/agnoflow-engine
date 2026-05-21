use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;
use wfe_core::{WfdPort, types::wfd::WFD};
use crate::{error::AppError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",             post(upload_wfd).get(list_wfd))
        .route("/:id/:version", get(get_wfd))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery { orgtnt_id: Uuid }

async fn list_wfd(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    wf_wfd::repo::list(&s.pool, q.orgtnt_id)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

#[derive(Deserialize)]
struct UploadBody {
    orgtnt_id: Uuid,
    #[serde(flatten)]
    wfd:       WFD,
}

async fn upload_wfd(
    State(s): State<AppState>,
    Json(body): Json<UploadBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (wfd_id, version) = s.wfd.upload(body.orgtnt_id, &body.wfd)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

async fn get_wfd(
    State(s): State<AppState>,
    Path((wfd_id, version)): Path<(Uuid, u32)>,
) -> Result<Json<WFD>, AppError> {
    s.wfd.fetch(wfd_id, version)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::NOT_FOUND))
}
