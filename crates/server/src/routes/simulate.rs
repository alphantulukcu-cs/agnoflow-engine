use std::sync::Arc;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::{actor::Actor, wfd::WFD};
use wf_wfe::{
    OrgAdapter, WfeExecutor,
    sim::{
        SimState,
        inline_wfd_port::InlineWfdPort,
        in_memory_wfe_port::InMemoryWfePort,
    },
};
use crate::{error::AppError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/start",            post(sim_start))
        .route("/apply",            post(sim_apply))
        .route("/possible-actions", post(sim_possible_actions))
        .with_state(state)
}

// ── /start ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimStartBody {
    wfd:   WFD,
    actor: Actor,
    #[serde(default)]
    input: Value,
}

#[derive(serde::Serialize)]
struct SimStartResponse {
    sim_state:        SimState,
    possible_actions: Vec<String>,
}

async fn sim_start(
    State(s): State<AppState>,
    Json(body): Json<SimStartBody>,
) -> Result<Json<SimStartResponse>, AppError> {
    let wfd      = body.wfd;
    let org      = Arc::new(OrgAdapter::new(s.pool.clone()));
    let wfd_port = Arc::new(InlineWfdPort::new(wfd.clone()));
    let wfe_port = Arc::new(InMemoryWfePort::new());
    let executor = WfeExecutor::new(org.clone(), wfd_port, wfe_port.clone());

    let result = executor
        .start(Uuid::nil(), 0, &body.actor, &body.input)
        .await
        .map_err(AppError::from)?;

    let wfes = wfe_port
        .get(result.wfe_id)
        .ok_or_else(|| AppError("sim state missing after start".into(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let sim_state = SimState::from_wfes(&wfes);

    let possible_actions = if sim_state.status == wfe_core::types::wfe::WfeStatus::Terminal {
        vec![]
    } else {
        let wfd_port2 = Arc::new(InlineWfdPort::new(wfd));
        let wfe_port2 = Arc::new(InMemoryWfePort::seeded(wfes));
        let executor2 = WfeExecutor::new(org, wfd_port2, wfe_port2);
        executor2
            .possible_actions(result.wfe_id, &body.actor)
            .await
            .map_err(AppError::from)?
    };

    Ok(Json(SimStartResponse { sim_state, possible_actions }))
}

// ── /apply ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimApplyBody {
    wfd:       WFD,
    sim_state: SimState,
    actor:     Actor,
    action:    String,
    #[serde(default)]
    input:     Value,
}

#[derive(serde::Serialize)]
struct SimApplyResponse {
    sim_state:        SimState,
    terminal:         bool,
    end_response:     Option<Value>,
    possible_actions: Vec<String>,
}

async fn sim_apply(
    State(s): State<AppState>,
    Json(body): Json<SimApplyBody>,
) -> Result<Json<SimApplyResponse>, AppError> {
    let wfe_id       = body.sim_state.wfe_id;
    let wfd          = body.wfd;
    let initial_wfes = body.sim_state.into_wfes();

    let org      = Arc::new(OrgAdapter::new(s.pool.clone()));
    let wfd_port = Arc::new(InlineWfdPort::new(wfd.clone()));
    let wfe_port = Arc::new(InMemoryWfePort::seeded(initial_wfes));
    let executor = WfeExecutor::new(org.clone(), wfd_port, wfe_port.clone());

    let result = executor
        .apply(wfe_id, &body.actor, &body.action, &body.input)
        .await
        .map_err(AppError::from)?;

    let wfes = wfe_port
        .get(wfe_id)
        .ok_or_else(|| AppError("sim state missing after apply".into(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let sim_state = SimState::from_wfes(&wfes);

    let possible_actions = if result.terminal {
        vec![]
    } else {
        let wfd_port2 = Arc::new(InlineWfdPort::new(wfd));
        let wfe_port2 = Arc::new(InMemoryWfePort::seeded(wfes));
        let executor2 = WfeExecutor::new(org, wfd_port2, wfe_port2);
        executor2
            .possible_actions(wfe_id, &body.actor)
            .await
            .map_err(AppError::from)?
    };

    Ok(Json(SimApplyResponse {
        sim_state,
        terminal:         result.terminal,
        end_response:     result.end_response,
        possible_actions,
    }))
}

// ── /possible-actions ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimPossibleActionsBody {
    wfd:       WFD,
    sim_state: SimState,
    actor:     Actor,
}

async fn sim_possible_actions(
    State(s): State<AppState>,
    Json(body): Json<SimPossibleActionsBody>,
) -> Result<Json<Vec<String>>, AppError> {
    let wfe_id       = body.sim_state.wfe_id;
    let initial_wfes = body.sim_state.into_wfes();

    let org      = Arc::new(OrgAdapter::new(s.pool.clone()));
    let wfd_port = Arc::new(InlineWfdPort::new(body.wfd));
    let wfe_port = Arc::new(InMemoryWfePort::seeded(initial_wfes));
    let executor = WfeExecutor::new(org, wfd_port, wfe_port);

    executor
        .possible_actions(wfe_id, &body.actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}
