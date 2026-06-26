// workflow-engine/crates/server/src/routes/portal/auth.rs

use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use crate::{error::AppError, state::AppState};
use super::jwt::{encode_jwt, PortalActor};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/login", post(login))
        .with_state(state)
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub role:     String,
    pub orgu_id:  Uuid,
}

#[derive(Serialize)]
pub struct LoginUser {
    pub id:        Uuid,
    pub full_name: String,
    pub role:      String,
    pub orgu_id:   Uuid,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub user:  LoginUser,
}

#[derive(FromRow)]
struct UserRow {
    u_id:          Uuid,
    full_name:     String,
    password_hash: Option<String>,
}

async fn login(
    State(s): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    // 1. Find user by username
    let row = sqlx::query_as::<_, UserRow>(
        "SELECT u_id, full_name, password_hash
         FROM org.u
         WHERE username = $1 AND is_active = true
         LIMIT 1"
    )
    .bind(&body.username)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("Kullanıcı adı veya şifre hatalı.".into(), StatusCode::UNAUTHORIZED))?;

    // 2. Verify password
    let hash = row.password_hash.as_deref().unwrap_or("");
    let ok = bcrypt::verify(&body.password, hash)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    if !ok {
        return Err(AppError("Kullanıcı adı veya şifre hatalı.".into(), StatusCode::UNAUTHORIZED));
    }

    // 3. Verify user is assigned to the requested orgu
    let orgu_assigned = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
             SELECT 1 FROM org.u_orgu
             WHERE u_id = $1 AND orgu_id = $2
         )"
    )
    .bind(row.u_id)
    .bind(body.orgu_id)
    .fetch_one(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    if !orgu_assigned {
        return Err(AppError("Bu birim için yetkiniz yok.".into(), StatusCode::FORBIDDEN));
    }

    // 4. Verify user has the requested role in that orgu
    let has_role = wf_org::repo::user_role::check_user_role(
        &s.pool, row.u_id, body.orgu_id, &body.role,
    )
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    if !has_role {
        return Err(AppError("Bu rol için yetkiniz yok.".into(), StatusCode::FORBIDDEN));
    }

    // 5. Get orgtnt_id for the orgu
    let orgtnt_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT orgtnt_id FROM org.orgt_orgu
         WHERE orgu_id = $1 AND is_active = true
         LIMIT 1"
    )
    .bind(body.orgu_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("Birim bulunamadı.".into(), StatusCode::BAD_REQUEST))?;

    // 6. Issue JWT (8 hour TTL)
    let actor = PortalActor {
        user_id:   row.u_id,
        orgu_id:   body.orgu_id,
        role:      body.role.clone(),
        orgtnt_id,
    };
    let token = encode_jwt(&s.cfg.jwt_secret, &actor, 8)?;

    Ok(Json(LoginResponse {
        token,
        user: LoginUser {
            id:        row.u_id,
            full_name: row.full_name,
            role:      body.role,
            orgu_id:   body.orgu_id,
        },
    }))
}
