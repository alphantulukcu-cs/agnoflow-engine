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
        .route("/usage-summary", get(usage_summary))
        .route("/draft", post(create_draft))
        .route("/draft/:id/:version", get(get_draft).put(save_draft).delete(delete_draft))
        .route("/draft/:id/:version/publish", post(publish_draft))
        .route("/:id/:version", get(get_wfd))
        .route("/:id/:version/new-draft", post(new_draft))
        .route("/:id/:version/usage", get(wfe_usage))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery {
    orgtnt_id: Uuid,
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_wfd(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    wf_wfd::repo::list(&s.pool, q.orgtnt_id, limit, offset)
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

#[derive(Deserialize)]
struct CreateDraftBody {
    orgtnt_id:   Uuid,
    name:        String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    tags:        Vec<String>,
    /// Editörün ürettiği başlangıç dokümanı; yoksa engine iskelet yazar.
    #[serde(default)]
    wfd:         Option<Value>,
}

async fn create_draft(
    State(s): State<AppState>,
    Json(b): Json<CreateDraftBody>,
) -> Result<Json<Value>, AppError> {
    let (wfd_id, version) = s.wfd
        .create_draft(b.orgtnt_id, &b.name, b.description.as_deref(), &b.tags, b.wfd.as_ref())
        .await
        .map_err(map_wfd_err)?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

async fn get_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    s.wfd.fetch_draft_json(id, ver).await.map(Json).map_err(map_wfd_err)
}

#[derive(Deserialize)]
struct SaveDraftBody {
    wfd:         Value,
    #[serde(default)]
    description: Option<String>,
    /// Verilmezse (None) mevcut tags korunur; boş `[]` gönderilirse temizlenir.
    #[serde(default)]
    tags:        Option<Vec<String>>,
}

async fn save_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
    Json(b): Json<SaveDraftBody>,
) -> Result<StatusCode, AppError> {
    s.wfd.save_draft(id, ver, &b.wfd, b.description.as_deref(), b.tags.as_deref())
        .await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

async fn publish_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    s.wfd.publish_draft(id, ver).await
        .map(|_| Json(serde_json::json!({ "wfd_id": id, "version": ver, "status": "published" })))
        .map_err(map_wfd_err)
}

async fn delete_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<StatusCode, AppError> {
    s.wfd.delete_draft(id, ver).await
        .map(|_| StatusCode::NO_CONTENT)
        .map_err(map_wfd_err)
}

async fn new_draft(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let (wfd_id, version) = s.wfd.new_draft_from(id, ver).await.map_err(map_wfd_err)?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

/// Bu published versiyonu kullanan WFE örneklerinin durum dağılımı.
/// `active` = anlık çalışan örnek sayısı.
async fn wfe_usage(
    State(s): State<AppState>,
    Path((id, ver)): Path<(Uuid, i32)>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, count(*)::bigint FROM wf.wfe \
         WHERE wfd_id = $1 AND wfd_version = $2 GROUP BY status",
    )
    .bind(id)
    .bind(ver)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let (mut active, mut terminal, mut error) = (0i64, 0i64, 0i64);
    for (status, n) in rows {
        match status.as_str() {
            "active" => active = n,
            "terminal" => terminal = n,
            "error" => error = n,
            _ => {}
        }
    }
    Ok(Json(serde_json::json!({
        "active": active,
        "terminal": terminal,
        "error": error,
        "total": active + terminal + error,
    })))
}

/// Tenant genelinde wfd_id başına anlık aktif WFE sayısı — dashboard özeti için
/// tek istekte tüm sayımları döner (satır başına /usage çağırmaya gerek kalmaz).
async fn usage_summary(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(Uuid, i64)> = sqlx::query_as(
        "SELECT wfd_id, count(*)::bigint FROM wf.wfe \
         WHERE orgtnt_id = $1 AND status = 'active' GROUP BY wfd_id",
    )
    .bind(q.orgtnt_id)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let arr: Vec<Value> = rows
        .into_iter()
        .map(|(wfd_id, active)| serde_json::json!({ "wfd_id": wfd_id, "active": active }))
        .collect();
    Ok(Json(serde_json::json!(arr)))
}

/// WfdError → HTTP kodu eşlemesi.
fn map_wfd_err(e: wf_wfd::error::WfdError) -> AppError {
    use wf_wfd::error::WfdError as E;
    let code = match e {
        E::NotFound(_)    => StatusCode::NOT_FOUND,
        E::Conflict(_)    => StatusCode::CONFLICT,
        E::InvalidJson(_) => StatusCode::UNPROCESSABLE_ENTITY,
        _                 => StatusCode::INTERNAL_SERVER_ERROR,
    };
    AppError(e.to_string(), code)
}
