use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use crate::{error::AppError, state::AppState};
use super::jwt::PortalActor;
use wfe_core::types::actor::Actor;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/:wfe_id",         get(get_wfe_detail))
        .route("/:wfe_id/action",  post(submit_action))
        .with_state(state)
}

/// Returns true if `claimed_by` JSON belongs to the given actor.
pub fn is_claimed_by(claimed: &Value, user_id: &str, orgu_id: &str, role: &str) -> bool {
    claimed.get("user_id").and_then(|v| v.as_str()).unwrap_or("") == user_id
        && claimed.get("orgu_id").and_then(|v| v.as_str()).unwrap_or("") == orgu_id
        && claimed.get("role").and_then(|v| v.as_str()).unwrap_or("") == role
}

#[derive(Debug, Serialize)]
struct ActionInputSchema {
    required: Vec<String>,
    optional: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AvailableAction {
    name:  String,
    input: ActionInputSchema,
}

#[derive(Debug, Serialize)]
struct ClaimedByInfo {
    user_id: String,
    orgu_id: String,
    role:    String,
}

#[derive(Debug, Serialize)]
struct WfeDetailResponse {
    wfe_id:            Uuid,
    wfd_name:          String,
    dynctx:            Value,
    claimed_by:        Option<ClaimedByInfo>,
    available_actions: Vec<AvailableAction>,
}

#[derive(sqlx::FromRow)]
struct WfeInfoRow {
    wfd_id:      Uuid,
    wfd_version: i32,
    claimed_by:  Option<Value>,
    wfd_name:    String,
}

async fn get_wfe_detail(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<WfeDetailResponse>, AppError> {
    let row = sqlx::query_as::<_, WfeInfoRow>(
        "SELECT e.wfd_id, e.wfd_version, e.claimed_by, m.name AS wfd_name
         FROM wf.wfe e
         JOIN wf.wfd_meta m ON m.wfd_id = e.wfd_id AND m.version = e.wfd_version
         WHERE e.wfe_id = $1 AND e.orgtnt_id = $2 AND e.status = 'active'"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı.".into(), StatusCode::NOT_FOUND))?;

    let claimed_by = row.claimed_by.as_ref().map(|cb| ClaimedByInfo {
        user_id: cb.get("user_id").and_then(|v| v.as_str()).unwrap_or("").into(),
        orgu_id: cb.get("orgu_id").and_then(|v| v.as_str()).unwrap_or("").into(),
        role:    cb.get("role").and_then(|v| v.as_str()).unwrap_or("").into(),
    });

    let portal_actor = Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role:    actor.role.clone(),
    };

    let view = s.executor
        .query(wfe_id, &portal_actor)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let action_names = s.executor
        .possible_actions(wfe_id, &portal_actor)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let wfd = s.executor.wfd
        .fetch(row.wfd_id, row.wfd_version as u32)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    let available_actions: Vec<AvailableAction> = action_names
        .iter()
        .filter_map(|name| {
            wfd.actions.get(name).map(|def| AvailableAction {
                name: name.clone(),
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
        dynctx:   view.dynctx,
        claimed_by,
        available_actions,
    }))
}

#[derive(Deserialize)]
struct ActionRequest {
    action: String,
    #[serde(default)]
    input:  Value,
}

#[derive(Serialize)]
struct ActionResponse {
    wfe_status: String,
}

#[derive(sqlx::FromRow)]
struct WfeCoreRow {
    wfd_id:      Uuid,
    wfd_version: i32,
    claimed_by:  Option<Value>,
}

async fn submit_action(
    State(s): State<AppState>,
    actor: PortalActor,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ActionRequest>,
) -> Result<Json<ActionResponse>, AppError> {
    let row = sqlx::query_as::<_, WfeCoreRow>(
        "SELECT wfd_id, wfd_version, claimed_by FROM wf.wfe
         WHERE wfe_id = $1 AND orgtnt_id = $2 AND status = 'active'"
    )
    .bind(wfe_id)
    .bind(actor.orgtnt_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("WFE bulunamadı veya aktif değil.".into(), StatusCode::NOT_FOUND))?;

    match &row.claimed_by {
        None => {
            return Err(AppError(
                "Bu görevi önce üstüne almalısınız.".into(),
                StatusCode::FORBIDDEN,
            ))
        }
        Some(cb) if !is_claimed_by(
            cb,
            &actor.user_id.to_string(),
            &actor.orgu_id.to_string(),
            &actor.role,
        ) => {
            return Err(AppError(
                "Bu görev başka biri tarafından alınmış.".into(),
                StatusCode::FORBIDDEN,
            ))
        }
        _ => {}
    }

    let portal_actor = Actor {
        orgu_id: actor.orgu_id,
        user_id: actor.user_id,
        role:    actor.role.clone(),
    };

    let action_names = s.executor
        .possible_actions(wfe_id, &portal_actor)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    if !action_names.contains(&body.action) {
        return Err(AppError(
            format!("'{}' aksiyonu şu an mevcut değil.", body.action),
            StatusCode::BAD_REQUEST,
        ));
    }

    let wfd = s.executor.wfd
        .fetch(row.wfd_id, row.wfd_version as u32)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    if let Some(action_def) = wfd.actions.get(&body.action) {
        let input_obj = body.input.as_object();
        for req_field in &action_def.input.required {
            let present = input_obj.map(|m| m.contains_key(req_field)).unwrap_or(false);
            if !present {
                return Err(AppError(
                    format!("Zorunlu alan eksik: '{req_field}'"),
                    StatusCode::BAD_REQUEST,
                ));
            }
        }
    }

    let result = s.executor
        .apply(wfe_id, &portal_actor, &body.action, &body.input)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?;

    sqlx::query("UPDATE wf.wfe SET claimed_by = NULL WHERE wfe_id = $1 AND orgtnt_id = $2")
        .bind(wfe_id)
        .bind(actor.orgtnt_id)
        .execute(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(ActionResponse {
        wfe_status: if result.terminal { "terminal".into() } else { "active".into() },
    }))
}

#[cfg(test)]
mod tests {
    use super::is_claimed_by;

    #[test]
    fn actor_matches_claimed_by() {
        let user_id = "550e8400-e29b-41d4-a716-446655440000";
        let orgu_id = "550e8400-e29b-41d4-a716-446655440001";
        let claimed = serde_json::json!({
            "user_id": user_id,
            "orgu_id": orgu_id,
            "role": "clerk"
        });
        assert!(is_claimed_by(&claimed, user_id, orgu_id, "clerk"));
        assert!(!is_claimed_by(&claimed, user_id, orgu_id, "manager"));
        assert!(!is_claimed_by(&claimed, "other-user", orgu_id, "clerk"));
    }
}
