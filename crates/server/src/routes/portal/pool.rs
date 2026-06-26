// workflow-engine/crates/server/src/routes/portal/pool.rs

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
use crate::{error::AppError, state::AppState};
use super::jwt::PortalActor;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",                    get(list_pool))
        .route("/:wfe_id/can-claim",   get(can_claim))
        .route("/:wfe_id/claim",       post(claim))
        .with_state(state)
}

/// One item in the pool list.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PoolTask {
    pub id:          Uuid,          // wfe_id
    pub title:       String,        // wfd_meta.name
    pub workflow_id: Uuid,          // wfd_id
    pub status:      String,
    pub created_at:  DateTime<Utc>,
}

/// Pure function — no I/O. Checks whether (orgu_id, role) appears in the
/// current_c_a JSONB array. Extracted for unit testing.
pub fn actor_in_current_ca(ca: &Value, orgu_id: Uuid, role: &str) -> bool {
    ca.as_array()
        .map(|arr| {
            arr.iter().any(|entry| {
                let eid = entry.get("orgu_id").and_then(|v| v.as_str())
                    .and_then(|s| Uuid::parse_str(s).ok());
                let erole = entry.get("role").and_then(|v| v.as_str()).unwrap_or("");
                eid == Some(orgu_id) && erole == role
            })
        })
        .unwrap_or(false)
}

async fn list_pool(
    State(s): State<AppState>,
    actor: PortalActor,
) -> Result<Json<Vec<PoolTask>>, AppError> {
    // JSONB containment filter — finds WFEs where actor is a candidate
    let ca_filter = serde_json::json!([{
        "orgu_id": actor.orgu_id.to_string(),
        "role":    actor.role
    }]);

    let tasks = sqlx::query_as::<_, PoolTask>(
        "SELECT e.wfe_id    AS id,
                m.name      AS title,
                e.wfd_id    AS workflow_id,
                e.status,
                e.created_at
         FROM wf.wfe e
         JOIN wf.wfd_meta m
           ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.status     = 'active'
           AND e.orgtnt_id  = $1
           AND e.current_c_a @> $2::jsonb
         ORDER BY e.created_at DESC"
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
}

async fn can_claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<CanClaimResponse>, AppError> {
    let ca: Option<Value> = sqlx::query_scalar::<_, Value>(
        "SELECT current_c_a FROM wf.wfe
         WHERE wfe_id = $1 AND status = 'active' AND orgtnt_id = $2"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let can = ca
        .as_ref()
        .map(|v| actor_in_current_ca(v, actor.orgu_id, &actor.role))
        .unwrap_or(false);

    Ok(Json(CanClaimResponse { can_claim: can }))
}

#[derive(Serialize)]
struct ClaimResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

async fn claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<ClaimResponse>, AppError> {
    // Re-check eligibility at claim time (guards race condition)
    let ca: Option<Value> = sqlx::query_scalar::<_, Value>(
        "SELECT current_c_a FROM wf.wfe
         WHERE wfe_id = $1 AND status = 'active' AND orgtnt_id = $2"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let can = ca
        .as_ref()
        .map(|v| actor_in_current_ca(v, actor.orgu_id, &actor.role))
        .unwrap_or(false);

    if can {
        Ok(Json(ClaimResponse { success: true, error: None }))
    } else {
        Ok(Json(ClaimResponse {
            success: false,
            error:   Some("İş alınamadı — yetki değişti veya iş kapandı.".into()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::actor_in_current_ca;
    use uuid::Uuid;
    use serde_json::json;

    #[test]
    fn actor_matches_ca_entry() {
        let orgu_id = Uuid::new_v4();
        let ca = json!([{"orgu_id": orgu_id.to_string(), "role": "clerk"}]);
        assert!(actor_in_current_ca(&ca, orgu_id, "clerk"));
    }

    #[test]
    fn actor_wrong_role_rejected() {
        let orgu_id = Uuid::new_v4();
        let ca = json!([{"orgu_id": orgu_id.to_string(), "role": "manager"}]);
        assert!(!actor_in_current_ca(&ca, orgu_id, "clerk"));
    }

    #[test]
    fn actor_wrong_orgu_rejected() {
        let orgu_id = Uuid::new_v4();
        let ca = json!([{"orgu_id": Uuid::new_v4().to_string(), "role": "clerk"}]);
        assert!(!actor_in_current_ca(&ca, orgu_id, "clerk"));
    }

    #[test]
    fn empty_ca_rejected() {
        let ca = json!([]);
        assert!(!actor_in_current_ca(&ca, Uuid::new_v4(), "clerk"));
    }
}
