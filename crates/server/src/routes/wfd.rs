use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", post(upload_wfd).get(list_wfd))
        .route("/validate", post(validate_wfd))
        .route("/:id/:version", get(get_wfd))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery {
    orgtnt_id: Uuid,
}

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
    /// v2.2 WFD dokümanı — yükleme kapısı + custom validator uygulanır (M14).
    wfd: Value,
}

async fn upload_wfd(
    State(s): State<AppState>,
    Json(body): Json<UploadBody>,
) -> Result<Json<Value>, AppError> {
    let (wfd_id, version) = s
        .wfd
        .upload(body.orgtnt_id, &body.wfd)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

/// Editör için: kaydetmeden doğrula — hata/uyarı listesi döner.
async fn validate_wfd(Json(wfd_json): Json<Value>) -> Result<Json<Value>, AppError> {
    let wfd = match wfe_core::types::wfd_v22::Wfd::from_value(wfd_json) {
        Ok(w) => w,
        Err(e) => {
            return Ok(Json(serde_json::json!({
                "valid": false,
                "errors": [{"code": "parse", "path": "$", "message": e.to_string()}],
                "warnings": [],
            })))
        }
    };
    let report = wfe_core::validator::validate(&wfd);
    let issue = |i: &wfe_core::validator::ValidationIssue| {
        serde_json::json!({"code": i.code, "path": i.path, "message": i.message})
    };
    Ok(Json(serde_json::json!({
        "valid": report.is_valid(),
        "errors": report.errors.iter().map(issue).collect::<Vec<_>>(),
        "warnings": report.warnings.iter().map(issue).collect::<Vec<_>>(),
    })))
}

async fn get_wfd(
    State(s): State<AppState>,
    Path((wfd_id, version)): Path<(Uuid, i32)>,
) -> Result<Json<wfe_core::types::wfd_v22::Wfd>, AppError> {
    s.wfd
        .fetch(wfd_id, version)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::NOT_FOUND))
}
