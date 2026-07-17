// Tasarım-zamanı kullanıcı auth'u (wf.app_user) — portal JWT'sinden (org.u) AYRI.
// Token minimal kimlik taşır (sub/orgtnt/role); proje yetkileri her istekte DB'den
// okunur. İleride Keycloak'a geçişte yalnız token doğrulama (AppAuth extractor +
// login ucu) değişir, yetki modeli aynı kalır.

use crate::{error::AppError, state::AppState};
use axum::{
    async_trait,
    extract::{FromRequestParts, State},
    http::{request::Parts, StatusCode},
    routing::{get, post},
    Json, Router,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/login", post(login))
        .route("/me", get(me))
        .with_state(state)
}

// ── JWT ──────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct AppClaims {
    sub: String,
    orgtnt_id: String,
    role: String,
    /// Portal token'larıyla karışmasın diye tip damgası.
    typ: String,
    exp: usize,
}

/// Doğrulanmış tasarım-zamanı kullanıcısı. Handler imzasına eklemek auth zorunlu kılar.
#[derive(Debug, Clone)]
pub struct AppAuth {
    pub user_id: Uuid,
    pub orgtnt_id: Uuid,
    pub role: String,
}

impl AppAuth {
    pub fn require_admin(&self) -> Result<(), AppError> {
        if self.role != "admin" {
            return Err(AppError(
                "Bu işlem tenant admin yetkisi gerektirir".into(),
                StatusCode::FORBIDDEN,
            ));
        }
        Ok(())
    }
}

pub fn encode_app_jwt(secret: &str, user_id: Uuid, orgtnt_id: Uuid, role: &str, ttl_hours: u64)
    -> Result<String, AppError>
{
    let exp = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + ttl_hours * 3600) as usize;
    let claims = AppClaims {
        sub: user_id.to_string(),
        orgtnt_id: orgtnt_id.to_string(),
        role: role.to_string(),
        typ: "app".into(),
        exp,
    };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| AppError(format!("JWT encode: {e}"), StatusCode::INTERNAL_SERVER_ERROR))
}

pub fn decode_app_jwt(secret: &str, token: &str) -> Result<AppAuth, AppError> {
    let data = decode::<AppClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError("Geçersiz veya süresi dolmuş token".into(), StatusCode::UNAUTHORIZED))?;
    let c = data.claims;
    if c.typ != "app" {
        return Err(AppError("Geçersiz token tipi".into(), StatusCode::UNAUTHORIZED));
    }
    Ok(AppAuth {
        user_id: Uuid::parse_str(&c.sub)
            .map_err(|_| AppError("Bad token: sub".into(), StatusCode::UNAUTHORIZED))?,
        orgtnt_id: Uuid::parse_str(&c.orgtnt_id)
            .map_err(|_| AppError("Bad token: orgtnt_id".into(), StatusCode::UNAUTHORIZED))?,
        role: c.role,
    })
}

#[async_trait]
impl FromRequestParts<AppState> for AppAuth {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| AppError(
                "Authorization: Bearer <token> required".into(),
                StatusCode::UNAUTHORIZED,
            ))?;
        decode_app_jwt(&state.cfg.jwt_secret, token)
    }
}

// ── Kullanıcı görünümü (login/me/users ortak şekli) ─────────────────────────

#[derive(Serialize)]
pub struct ProjectMembership {
    pub project_id: Uuid,
    pub project_name: String,
    pub role: String,
}

#[derive(Serialize)]
pub struct UserView {
    pub user_id: Uuid,
    pub orgtnt_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub projects: Vec<ProjectMembership>,
}

pub async fn load_memberships(pool: &sqlx::PgPool, user_id: Uuid)
    -> Result<Vec<ProjectMembership>, AppError>
{
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT m.project_id, p.name, m.role
         FROM wf.project_member m JOIN wf.project p USING (project_id)
         WHERE m.user_id = $1 ORDER BY p.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(rows
        .into_iter()
        .map(|(project_id, project_name, role)| ProjectMembership { project_id, project_name, role })
        .collect())
}

#[derive(sqlx::FromRow)]
pub struct UserRow {
    pub user_id: Uuid,
    pub orgtnt_id: Uuid,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub const USER_COLS: &str =
    "user_id, orgtnt_id, email, display_name, role, is_active, created_at";

pub async fn user_view(pool: &sqlx::PgPool, row: UserRow) -> Result<UserView, AppError> {
    let projects = load_memberships(pool, row.user_id).await?;
    Ok(UserView {
        user_id: row.user_id,
        orgtnt_id: row.orgtnt_id,
        email: row.email,
        display_name: row.display_name,
        role: row.role,
        is_active: row.is_active,
        created_at: row.created_at,
        projects,
    })
}

// ── Handlers ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginBody {
    orgtnt_id: Uuid,
    email: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResponse {
    token: String,
    user: UserView,
}

async fn login(
    State(s): State<AppState>,
    Json(body): Json<LoginBody>,
) -> Result<Json<LoginResponse>, AppError> {
    let bad = || AppError("E-posta veya şifre hatalı".into(), StatusCode::UNAUTHORIZED);

    let user = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLS} FROM wf.app_user \
         WHERE orgtnt_id = $1 AND lower(email) = lower($2) AND is_active = true",
    ))
    .bind(body.orgtnt_id)
    .bind(&body.email)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(bad)?;

    let hash: String = sqlx::query_scalar(
        "SELECT password_hash FROM wf.app_user WHERE user_id = $1",
    )
    .bind(user.user_id)
    .fetch_one(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;

    if !bcrypt::verify(&body.password, &hash)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    {
        return Err(bad());
    }

    let token = encode_app_jwt(&s.cfg.jwt_secret, user.user_id, user.orgtnt_id, &user.role, 12)?;
    let user = user_view(&s.pool, user).await?;
    Ok(Json(LoginResponse { token, user }))
}

/// Token'daki kimlikle güncel kullanıcı durumu (rol/atama değişmiş olabilir).
async fn me(State(s): State<AppState>, auth: AppAuth) -> Result<Json<UserView>, AppError> {
    let row = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLS} FROM wf.app_user WHERE user_id = $1 AND is_active = true",
    ))
    .bind(auth.user_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("Kullanıcı bulunamadı".into(), StatusCode::UNAUTHORIZED))?;
    user_view(&s.pool, row).await.map(Json)
}
