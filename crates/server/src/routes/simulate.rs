//! Simülasyon endpoint'leri — editör WFD'yi kaydetmeden dener.
//! v2.2: Engine saf olduğundan store yok; SimState istemciyle gidip gelir.
//! Claim YAZIMI simülasyonda atlanır (apply öncesi state aktöre atanır) ama
//! claim UYGUNLUĞU atlanmaz: gerçek akıştaki matcher'ın aynısı (Engine::can_claim)
//! her possible-actions/apply'da denetlenir — aksi halde sim'de herkes her şeyi
//! yapabilir ve sim gerçek davranışı yanlış gösterir.

use utoipa_axum::router::OpenApiRouter;
use crate::{error::AppError, state::AppState};
use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;
use wf_wfe::{
    executor::{active_branch_nodes, PossibleAction},
    sim::SimState,
    LiveAutoexecRunner, OrgAdapter,
};
use wfe_core::types::actor::Actor;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::validator;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(sim_start))
        .routes(routes!(sim_apply))
        .routes(routes!(sim_possible_actions))
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

#[derive(Deserialize, ToSchema)]
struct SimStartBody {
    wfd: Value,
    #[schema(value_type = Object)]
    actor: Actor,
    /// M16: verilirse yalnız bu action adını taşıyan start kuralları aday olur.
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    input: Value,
}

/// Sim claim-eşdeğeri uygunluk — gerçek `WfeExecutor::can_claim` ile aynı kural:
/// zaten sahipse uygun; değilse `Engine::can_claim` (matcher §7.1, delegation dahil).
/// Sahiplik ATANMAMIŞ wfes üzerinde denetlenir (sim'in geçici pre-claim'i yetkiyi
/// gölgelemesin diye).
async fn sim_eligible(
    engine: &Engine<'_>,
    wfd: &Wfd,
    sim_state: &SimState,
    actor: &Actor,
    node: Option<&str>,
) -> Result<bool, AppError> {
    let wfes = sim_state.to_wfes(None);
    let owned = match node {
        Some(n) => wfes
            .branches
            .iter()
            .any(|b| b.branch_node == n && b.claimed_by == Some(actor.user_id)),
        None => wfes.assigned_to == Some(actor.user_id),
    };
    if owned {
        return Ok(true);
    }
    let check = engine
        .can_claim(wfd, &wfes, actor, node)
        .await
        .map_err(AppError::from)?;
    Ok(matches!(check, ClaimCheck::Ok))
}

/// Sim aksiyon listesi — `possible_actions_for`'un uygunluk-farkındalı ikizi:
/// yalnız aktörün claim-uygun olduğu node/kolların aksiyonları döner.
async fn sim_actions_for(
    engine: &Engine<'_>,
    wfd: &Wfd,
    sim_state: &SimState,
    actor: &Actor,
) -> Result<Vec<PossibleAction>, AppError> {
    let wfes_owned = sim_state.to_wfes(Some(actor.user_id));
    if wfes_owned.join_target.is_some() {
        let mut out = Vec::new();
        for node in active_branch_nodes(&wfes_owned) {
            if !sim_eligible(engine, wfd, sim_state, actor, Some(&node)).await? {
                continue;
            }
            let actions = engine
                .possible_actions(wfd, &wfes_owned, actor, Some(&node))
                .await
                .map_err(AppError::from)?;
            out.extend(actions.into_iter().map(|action| PossibleAction {
                action,
                node: Some(node.clone()),
            }));
        }
        Ok(out)
    } else {
        if !sim_eligible(engine, wfd, sim_state, actor, None).await? {
            return Ok(vec![]);
        }
        let actions = engine
            .possible_actions(wfd, &wfes_owned, actor, None)
            .await
            .map_err(AppError::from)?;
        Ok(actions
            .into_iter()
            .map(|action| PossibleAction { action, node: None })
            .collect())
    }
}

#[derive(serde::Serialize, ToSchema)]
struct SimStartResponse {
    #[schema(value_type = Object)]
    sim_state: SimState,
    #[schema(value_type = Vec<Object>)]
    possible_actions: Vec<PossibleAction>,
}

#[utoipa::path(post, path = "/start", tag = "simulate",
    request_body = SimStartBody,
    responses((status = 200, description = "Sim başlangıç durumu + olası aksiyonlar", body = SimStartResponse)))]
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

    let possible_actions = sim_actions_for(&engine, &wfd, &sim_state, &body.actor).await?;

    Ok(Json(SimStartResponse {
        sim_state,
        possible_actions,
    }))
}

// ── /apply ───────────────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
struct SimApplyBody {
    wfd: Value,
    #[schema(value_type = Object)]
    sim_state: SimState,
    #[schema(value_type = Object)]
    actor: Actor,
    action: String,
    #[serde(default)]
    input: Value,
    /// WOR-31 T4: paralel modda kol seçimi (bkz. `routes/wfe.rs::ApplyBody.node`).
    #[serde(default)]
    node: Option<String>,
}

#[derive(serde::Serialize, ToSchema)]
struct SimApplyResponse {
    #[schema(value_type = Object)]
    sim_state: SimState,
    terminal: bool,
    end_response: Option<Value>,
    #[schema(value_type = Vec<Object>)]
    possible_actions: Vec<PossibleAction>,
}

#[utoipa::path(post, path = "/apply", tag = "simulate",
    request_body = SimApplyBody,
    responses((status = 200, description = "Aksiyon sonrası sim durumu", body = SimApplyResponse)))]
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

    // Claim YAZIMI atlanır ama uygunluk atlanmaz: gerçek akışta bu aktör claim
    // alamayacaksa sim'de de aksiyon uygulayamamalı (yetki delik olmasın).
    if !sim_eligible(&engine, &wfd, &sim_state, &body.actor, body.node.as_deref()).await? {
        return Err(AppError(
            "aktör bu adım için yetkili değil (c_a eşleşmiyor) — gerçek akışta claim reddedilirdi"
                .into(),
            StatusCode::FORBIDDEN,
        ));
    }

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
        sim_actions_for(&engine, &wfd, &sim_state, &body.actor).await?
    };

    Ok(Json(SimApplyResponse {
        end_response: sim_state.end_response.clone(),
        sim_state,
        terminal,
        possible_actions,
    }))
}

// ── /possible-actions ────────────────────────────────────────────────────────

#[derive(Deserialize, ToSchema)]
struct SimPossibleActionsBody {
    wfd: Value,
    #[schema(value_type = Object)]
    sim_state: SimState,
    #[schema(value_type = Object)]
    actor: Actor,
}

#[utoipa::path(post, path = "/possible-actions", tag = "simulate",
    request_body = SimPossibleActionsBody,
    responses((status = 200, description = "Aktörün claim-uygun olduğu olası aksiyonlar", body = serde_json::Value)))]
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

    sim_actions_for(&engine, &wfd, &body.sim_state, &body.actor)
        .await
        .map(Json)
}
