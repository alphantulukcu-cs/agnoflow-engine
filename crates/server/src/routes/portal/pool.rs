use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedByInfo {
    pub user_id: String,
    pub orgu_id: String,
    pub role:    String,
}

/// One item in the pool list.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct PoolTask {
    pub id:          Uuid,
    pub title:       String,
    pub workflow_id: Uuid,
    pub status:      String,
    pub created_at:  DateTime<Utc>,
    pub claimed_by:  Option<Value>,
}

/// Pure function — no I/O. Checks c_a membership and claimed_by state.
/// Returns (can_claim, reason).
pub fn compute_can_claim(
    ca:         &Value,
    claimed_by: &Option<Value>,
    orgu_id:    Uuid,
    user_id:    Uuid,
    role:       &str,
) -> (bool, Option<String>) {
    if let Some(cb) = claimed_by {
        let cb_user = cb.get("user_id").and_then(|v| v.as_str()).unwrap_or("");
        if cb_user != user_id.to_string() {
            return (false, Some("already_claimed".into()));
        }
        return (true, None);
    }
    if actor_in_current_ca(ca, orgu_id, role) {
        (true, None)
    } else {
        (false, Some("not_eligible".into()))
    }
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

#[derive(sqlx::FromRow)]
struct ClaimRow {
    current_c_a: Value,
    claimed_by:  Option<Value>,
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
        "SELECT e.wfe_id    AS id,
                m.name      AS title,
                e.wfd_id    AS workflow_id,
                e.status,
                e.created_at,
                e.claimed_by
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
    #[serde(skip_serializing_if = "Option::is_none")]
    reason:    Option<String>,
}

async fn can_claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<CanClaimResponse>, AppError> {
    let row = sqlx::query_as::<_, ClaimRow>(
        "SELECT current_c_a, claimed_by FROM wf.wfe
         WHERE wfe_id = $1 AND status = 'active' AND orgtnt_id = $2"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let Some(row) = row else {
        return Ok(Json(CanClaimResponse { can_claim: false, reason: Some("not_eligible".into()) }));
    };

    let (can_claim, reason) = compute_can_claim(
        &row.current_c_a,
        &row.claimed_by,
        actor.orgu_id,
        actor.user_id,
        &actor.role,
    );

    Ok(Json(CanClaimResponse { can_claim, reason }))
}

#[derive(Serialize)]
struct ClaimResponse {
    success: bool,
}

async fn claim(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<ClaimResponse>, AppError> {
    let row = sqlx::query_as::<_, ClaimRow>(
        "SELECT current_c_a, claimed_by FROM wf.wfe
         WHERE wfe_id = $1 AND status = 'active' AND orgtnt_id = $2"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    let (can, reason) = compute_can_claim(
        &row.current_c_a,
        &row.claimed_by,
        actor.orgu_id,
        actor.user_id,
        &actor.role,
    );

    if !can {
        let status = match reason.as_deref() {
            Some("already_claimed") => StatusCode::CONFLICT,
            _ => StatusCode::FORBIDDEN,
        };
        return Err(AppError(
            reason.unwrap_or_else(|| "Bu görevi almak için yetkiniz yok.".into()),
            status,
        ));
    }

    let claimed_by_json = serde_json::json!({
        "user_id": actor.user_id.to_string(),
        "orgu_id": actor.orgu_id.to_string(),
        "role":    actor.role
    });

    sqlx::query("UPDATE wf.wfe SET claimed_by = $1, updated_at = now() WHERE wfe_id = $2 AND orgtnt_id = $3")
        .bind(claimed_by_json)
        .bind(wfe_id)
        .bind(actor.orgtnt_id)
        .execute(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(ClaimResponse { success: true }))
}

#[cfg(test)]
mod tests {
    use super::{actor_in_current_ca, compute_can_claim};
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

    #[test]
    fn can_claim_reason_already_claimed() {
        let orgu_id = Uuid::new_v4();
        let other_user = Uuid::new_v4();
        let actor_user = Uuid::new_v4();
        let ca = json!([{"orgu_id": orgu_id.to_string(), "role": "clerk"}]);
        let claimed = json!({
            "user_id": other_user.to_string(),
            "orgu_id": orgu_id.to_string(),
            "role": "clerk"
        });
        let (can, reason) = compute_can_claim(&ca, &Some(claimed), orgu_id, actor_user, "clerk");
        assert!(!can);
        assert_eq!(reason.as_deref(), Some("already_claimed"));
    }

    #[test]
    fn can_claim_reason_not_eligible() {
        let orgu_id = Uuid::new_v4();
        let actor_user = Uuid::new_v4();
        let ca = json!([{"orgu_id": Uuid::new_v4().to_string(), "role": "manager"}]);
        let (can, reason) = compute_can_claim(&ca, &None, orgu_id, actor_user, "clerk");
        assert!(!can);
        assert_eq!(reason.as_deref(), Some("not_eligible"));
    }

    #[test]
    fn can_claim_success() {
        let orgu_id = Uuid::new_v4();
        let actor_user = Uuid::new_v4();
        let ca = json!([{"orgu_id": orgu_id.to_string(), "role": "clerk"}]);
        let (can, reason) = compute_can_claim(&ca, &None, orgu_id, actor_user, "clerk");
        assert!(can);
        assert!(reason.is_none());
    }

    #[test]
    fn can_claim_already_mine_ok() {
        let orgu_id = Uuid::new_v4();
        let actor_user = Uuid::new_v4();
        let ca = json!([{"orgu_id": orgu_id.to_string(), "role": "clerk"}]);
        let claimed = json!({
            "user_id": actor_user.to_string(),
            "orgu_id": orgu_id.to_string(),
            "role": "clerk"
        });
        let (can, reason) = compute_can_claim(&ca, &Some(claimed), orgu_id, actor_user, "clerk");
        assert!(can);
        assert!(reason.is_none());
    }
}
