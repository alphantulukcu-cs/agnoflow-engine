//! Simülasyon endpoint'leri — editör WFD'yi kaydetmeden dener.
//! v2.2: Engine saf olduğundan store yok; SimState istemciyle gidip gelir.
//! Claim akışı simülasyonda atlanır (apply öncesi state aktöre atanır).

use crate::{error::AppError, state::AppState};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;
use wf_wfe::{executor::PossibleAction, sim::SimState, LiveAutoexecRunner, OrgAdapter};
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::pipeline::Engine;
use wfe_core::validator;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/start", post(sim_start))
        .route("/apply", post(sim_apply))
        .route("/possible-actions", post(sim_possible_actions))
        .with_state(state)
}

fn parse_and_validate(wfd_json: Value) -> Result<Wfd, AppError> {
    let wfd = Wfd::from_value(wfd_json)
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    let report = validator::validate(&wfd);
    if !report.is_valid() {
        let summary = report
            .errors
            .iter()
            .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(AppError(
            format!("WFD geçersiz: {summary}"),
            StatusCode::UNPROCESSABLE_ENTITY,
        ));
    }
    Ok(wfd)
}

// ── /start ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimStartBody {
    wfd: Value,
    actor: Actor,
    /// M16: verilirse yalnız bu action adını taşıyan start kuralları aday olur.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
}

#[derive(serde::Serialize)]
struct SimStartResponse {
    sim_state: SimState,
    possible_actions: Vec<PossibleAction>,
}

async fn sim_start(
    State(s): State<AppState>,
    Json(body): Json<SimStartBody>,
) -> Result<Json<SimStartResponse>, AppError> {
    let wfd = parse_and_validate(body.wfd)?;
    let org = Arc::new(OrgAdapter::new(s.pool.clone()));
    let runner = LiveAutoexecRunner::new(Some(s.pool.clone()));
    let engine = Engine {
        org: &*org,
        exec: &runner,
    };

    let orgtnt_id = wfe_core::OrgPort::orgtnt_for_orgu(&*org, body.actor.orgu_id)
        .await
        .map_err(AppError::from)?;

    let new = engine
        .start(
            &wfd,
            &body.actor,
            orgtnt_id,
            body.action.as_deref(),
            &body.input,
            Uuid::new_v4(),
            None, // simülasyonda SLA-3 deadline izlenmez
        )
        .await
        .map_err(AppError::from)?;
    let sim_state = SimState::from_new_wfe(&new);

    let wfes = sim_state.to_wfes(Some(body.actor.user_id));
    let possible_actions = wf_wfe::executor::possible_actions_for(&engine, &wfd, &wfes, &body.actor)
        .await
        .map_err(AppError::from)?;

    Ok(Json(SimStartResponse {
        sim_state,
        possible_actions,
    }))
}

// ── /apply ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimApplyBody {
    wfd: Value,
    sim_state: SimState,
    actor: Actor,
    action: String,
    #[serde(default)]
    input: Value,
    /// WOR-31 T4: paralel modda kol seçimi (bkz. `routes/wfe.rs::ApplyBody.node`).
    #[serde(default)]
    node: Option<String>,
}

#[derive(serde::Serialize)]
struct SimApplyResponse {
    sim_state: SimState,
    terminal: bool,
    end_response: Option<Value>,
    possible_actions: Vec<PossibleAction>,
}

async fn sim_apply(
    State(s): State<AppState>,
    Json(body): Json<SimApplyBody>,
) -> Result<Json<SimApplyResponse>, AppError> {
    let wfd = parse_and_validate(body.wfd)?;
    let org = Arc::new(OrgAdapter::new(s.pool.clone()));
    let runner = LiveAutoexecRunner::new(Some(s.pool.clone()));
    let engine = Engine {
        org: &*org,
        exec: &runner,
    };

    let mut sim_state = body.sim_state;
    // simülasyon claim'i atlar — state uygulanmadan önce aktöre atanır
    let wfes = sim_state.to_wfes(Some(body.actor.user_id));

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &body.actor,
            &body.action,
            &body.input,
            body.node.as_deref(),
        )
        .await
        .map_err(AppError::from)?;
    sim_state.apply_commit(&commit);

    let terminal = matches!(
        sim_state.status,
        wfe_core::types::wfe::WfeStatus::Terminal | wfe_core::types::wfe::WfeStatus::Terminated
    );
    let possible_actions = if terminal {
        vec![]
    } else {
        let wfes = sim_state.to_wfes(Some(body.actor.user_id));
        wf_wfe::executor::possible_actions_for(&engine, &wfd, &wfes, &body.actor)
            .await
            .map_err(AppError::from)?
    };

    Ok(Json(SimApplyResponse {
        end_response: sim_state.end_response.clone(),
        sim_state,
        terminal,
        possible_actions,
    }))
}

// ── /possible-actions ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SimPossibleActionsBody {
    wfd: Value,
    sim_state: SimState,
    actor: Actor,
}

async fn sim_possible_actions(
    State(s): State<AppState>,
    Json(body): Json<SimPossibleActionsBody>,
) -> Result<Json<Vec<PossibleAction>>, AppError> {
    let wfd = parse_and_validate(body.wfd)?;
    let org = Arc::new(OrgAdapter::new(s.pool.clone()));
    let runner = LiveAutoexecRunner::new(Some(s.pool.clone()));
    let engine = Engine {
        org: &*org,
        exec: &runner,
    };

    let wfes = body.sim_state.to_wfes(Some(body.actor.user_id));
    wf_wfe::executor::possible_actions_for(&engine, &wfd, &wfes, &body.actor)
        .await
        .map(Json)
        .map_err(AppError::from)
}
