//! Portal WFE detay + aksiyon endpoint'leri (WOR-44 — v2.2).
//! Assignment/permission kontrolleri engine pipeline'ındadır (§7.1-7.2);
//! burada yalnızca görünüm zenginleştirme yapılır.

use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use wfe_core::v22::ports::WfdStore;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/:wfe_id", get(get_wfe_detail))
        .route("/:wfe_id/action", post(submit_action))
        .with_state(state)
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

#[derive(Debug, Serialize)]
struct ActionInputSchema {
    required: Vec<String>,
    optional: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AvailableAction {
    name: String,
    label: Option<String>,
    input: ActionInputSchema,
}

#[derive(Debug, Serialize)]
struct WfeDetailResponse {
    wfe_id: Uuid,
    wfd_name: String,
    current_node: Option<String>,
    node_label: Option<String>,
    dynctx: Value,
    claimed_by: Option<Uuid>,
    available_actions: Vec<AvailableAction>,
}

#[derive(sqlx::FromRow)]
struct WfeInfoRow {
    wfd_id: Uuid,
    wfd_version: i32,
    wfd_name: String,
}

async fn get_wfe_detail(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<WfeDetailResponse>, AppError> {
    let row = sqlx::query_as::<_, WfeInfoRow>(
        "SELECT e.wfd_id, e.wfd_version, m.name AS wfd_name
         FROM wf.wfe e
         JOIN wf.wfd_meta m ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.wfe_id = $1 AND e.orgtnt_id = $2 AND e.status = 'active'",
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    let portal_actor = to_actor(&actor);

    let view = s
        .executor
        .query(wfe_id, &portal_actor)
        .await
        .map_err(AppError::from)?;
    let action_names = s
        .executor
        .possible_actions(wfe_id, &portal_actor)
        .await
        .map_err(AppError::from)?;

    let wfd = s
        .wfd
        .fetch(row.wfd_id, row.wfd_version)
        .await
        .map_err(AppError::from)?;

    let node_label = view
        .current_node
        .as_ref()
        .and_then(|n| wfd.nodes.get(n))
        .and_then(|n| n.label.clone());

    let available_actions: Vec<AvailableAction> = action_names
        .iter()
        .filter_map(|name| {
            wfd.actions.get(name).map(|def| AvailableAction {
                name: name.clone(),
                label: def.label.clone(),
                input: ActionInputSchema {
                    required: def.input.required.clone(),
                    optional: def.input.optional.clone(),
                },
            })
        })
        .collect();

    Ok(Json(WfeDetailResponse {
        wfe_id,
        wfd_name: row.wfd_name,
        current_node: view.current_node,
        node_label,
        dynctx: view.dynctx,
        claimed_by: view.claimed_by,
        available_actions,
    }))
}

#[derive(Deserialize)]
struct ActionRequest {
    action: String,
    #[serde(default)]
    input: Value,
}

#[derive(Serialize)]
struct ActionResponse {
    wfe_status: String,
    current_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_response: Option<Value>,
}

async fn submit_action(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, AppError> {
    // Assignment, yetki, input validasyonu ve claimed_by reset'i engine +
    // atomik commit içinde — burada ek SQL yok (WOR-43).
    let result = s
        .executor
        .apply(wfe_id, &to_actor(&actor), &body.action, &body.input)
        .await
        .map_err(AppError::from)?;

    Ok(Json(ActionResponse {
        wfe_status: if result.terminal { "terminal".into() } else { "active".into() },
        current_node: result.current_node,
        end_response: result.end_response,
    }))
}
