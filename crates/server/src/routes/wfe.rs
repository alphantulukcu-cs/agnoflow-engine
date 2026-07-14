use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", post(start_wfe).get(list_wfe))
        .route("/:id/actions", post(apply_action))
        .route("/:id", get(query_wfe))
        .route("/:id/claim", post(claim_wfe))
        .route("/:id/possible-actions", get(possible_actions))
        .with_state(state)
}

fn extract_actor(headers: &HeaderMap) -> Result<Actor, AppError> {
    let orgu_id = parse_uuid_header(headers, "x-actor-orgu")?;
    let user_id = parse_uuid_header(headers, "x-actor-user")?;
    let role = headers
        .get("x-actor-role")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            AppError(
                "X-Actor-Role header required".into(),
                StatusCode::BAD_REQUEST,
            )
        })?
        .to_string();
    Ok(Actor {
        orgu_id,
        user_id,
        role,
    })
}

fn parse_uuid_header(headers: &HeaderMap, name: &str) -> Result<Uuid, AppError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| {
            AppError(
                format!("{name} header required (UUID)"),
                StatusCode::BAD_REQUEST,
            )
        })
}

#[derive(Deserialize)]
struct StartBody {
    wfd_id: Uuid,
    version: i32,
    /// M16: start aksiyonları gerçek ad taşır — verilirse yalnız bu action adını
    /// taşıyan start kuralları aday olur; verilmezse tüm start kuralları denenir.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
}

async fn start_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<StartBody>,
) -> Result<Json<wf_wfe::executor::WfeStartResult>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .start(body.wfd_id, body.version, &actor, body.action.as_deref(), &body.input)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize)]
struct ApplyBody {
    action: String,
    #[serde(default)]
    input: Value,
}

async fn apply_action(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<wf_wfe::executor::WfeApplyResult>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .apply(wfe_id, &actor, &body.action, &body.input)
        .await
        .map(Json)
        .map_err(AppError::from)
}

async fn claim_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<wf_wfe::executor::ClaimOutcome>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .claim(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

async fn query_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<wf_wfe::executor::WfeView>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .query(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

async fn possible_actions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<String>>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .possible_actions(wfe_id, &actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize)]
struct WfeListQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_wfe(
    State(s): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<WfeListQuery>,
) -> Result<Json<Vec<wf_wfe::models::WfeRow>>, AppError> {
    let actor = extract_actor(&headers)?;
    // WOR-5 fix: orgu_id tenant DEĞİLDİR — orgtnt_id org katmanından çözülür
    let orgtnt_id = s
        .executor
        .org
        .orgtnt_for_orgu(actor.orgu_id)
        .await
        .map_err(AppError::from)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    wf_wfe::repo::wfe::list_by_tenant(&s.pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}
