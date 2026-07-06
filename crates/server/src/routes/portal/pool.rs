//! Portal havuz endpoint'leri (WOR-44 — v2.2 uyumu).
//! Listeleme denormalize current_c_a cache'i ile SQL'de; can-claim/claim
//! kararı engine matcher'ı ile verilir (c_u kuralları dahil), yazım CAS'tır.

use super::jwt::PortalActor;
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list_pool))
        .route("/:wfe_id/can-claim", get(can_claim))
        .route("/:wfe_id/claim", post(claim))
        .with_state(state)
}

fn to_actor(actor: &PortalActor) -> Actor {
    Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role: actor.role.clone(),
    }
}

/// One item in the pool list.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PoolTask {
    pub id: Uuid,
    pub title: String,
    pub workflow_id: Uuid,
    pub status: String,
    pub current_node: Option<String>,
    pub created_at: DateTime<Utc>,
    pub claimed_by: Option<Value>,
}

async fn list_pool(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<PoolTask>>, AppError> {
    let ca_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "role":    actor.role
    }]);

    let tasks = sqlx::query_as::<_, PoolTask>(
        "SELECT e.wfe_id       AS id,
                m.name         AS title,
                e.wfd_id       AS workflow_id,
                e.status,
                e.current_node,
                e.created_at,
                e.claimed_by
         FROM wf.wfe e
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND e.current_c_a @> $2::jsonb
         ORDER BY e.created_at DESC",
    )
    .bind(actor.orgtnt_id)
    .bind(ca_filter)
    .fetch_all(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(tasks))
}

#[derive(Serialize)]
struct CanClaimResponse {
    can_claim: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

async fn can_claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<CanClaimResponse>, AppError> {
    let (can_claim, reason) = s
        .executor
        .can_claim(wfe_id, &to_actor(&actor))
        .await
        .map_err(AppError::from)?;
    Ok(Json(CanClaimResponse { can_claim, reason }))
}

#[derive(Serialize)]
struct ClaimResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

async fn claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<ClaimResponse>, AppError> {
    let outcome = s
        .executor
        .claim(wfe_id, &to_actor(&actor))
        .await
        .map_err(AppError::from)?;
    if !outcome.success {
        let status = match outcome.reason.as_deref() {
            Some("already_claimed") => StatusCode::CONFLICT,
            _ => StatusCode::FORBIDDEN,
        };
        return Err(AppError(
            outcome
                .reason
                .unwrap_or_else(|| "Bu görevi almak için yetkiniz yok.".into()),
            status,
        ));
    }
    Ok(Json(ClaimResponse {
        success: true,
        reason: None,
    }))
}
