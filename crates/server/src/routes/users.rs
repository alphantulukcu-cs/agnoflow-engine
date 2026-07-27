// Kullanıcı yönetimi — yalnız tenant admin (AppAuth.require_admin).
// Değişmez kural: bir tenant'ta son aktif admin silinemez, pasifleştirilemez, düşürülemez.

use utoipa_axum::router::OpenApiRouter;
use super::auth::{load_memberships, user_view, AppAuth, UserRow, UserView, USER_COLS};
use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use utoipa::ToSchema;
use utoipa_axum::routes;
use uuid::Uuid;

pub fn router(state: AppState) -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(list_users, create_user))
        .routes(routes!(update_user, delete_user))
        .routes(routes!(set_projects))
        .with_state(state)
}

const BCRYPT_COST: u32 = 10;

fn internal(e: impl std::fmt::Display) -> AppError {
    AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
}

/// Hedef kullanıcıyı admin'in tenant'ında bulur — cross-tenant erişimi 404'a düşürür.
async fn fetch_target(
    pool: &sqlx::PgPool,
    auth: &AppAuth,
    user_id: Uuid,
) -> Result<UserRow, AppError> {
    sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLS} FROM wf.app_user WHERE user_id = $1 AND orgtnt_id = $2",
    ))
    .bind(user_id)
    .bind(auth.orgtnt_id)
    .fetch_optional(pool)
    .await
    .map_err(internal)?
    .ok_or_else(|| AppError("Kullanıcı bulunamadı".into(), StatusCode::NOT_FOUND))
}

async fn active_admin_count(pool: &sqlx::PgPool, orgtnt_id: Uuid) -> Result<i64, AppError> {
    sqlx::query_scalar(
        "SELECT count(*)::bigint FROM wf.app_user \
         WHERE orgtnt_id = $1 AND role = 'admin' AND is_active = true",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await
    .map_err(internal)
}

/// Hedef aktif bir admin ise ve tenant'ta başka aktif admin yoksa işlemi reddeder.
async fn guard_last_admin(pool: &sqlx::PgPool, target: &UserRow) -> Result<(), AppError> {
    if target.role == "admin"
        && target.is_active
        && active_admin_count(pool, target.orgtnt_id).await? <= 1
    {
        return Err(AppError(
            "Tenant'ın son aktif admin'i silinemez/düşürülemez".into(),
            StatusCode::CONFLICT,
        ));
    }
    Ok(())
}

#[utoipa::path(get, path = "/", tag = "users",
    responses((status = 200, description = "Tenant kullanıcıları", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn list_users(
    State(s): State<AppState>,
    auth: AppAuth,
) -> Result<Json<Vec<UserView>>, AppError> {
    auth.require_admin()?;
    let rows = sqlx::query_as::<_, UserRow>(&format!(
        "SELECT {USER_COLS} FROM wf.app_user WHERE orgtnt_id = $1 ORDER BY created_at",
    ))
    .bind(auth.orgtnt_id)
    .fetch_all(&s.pool)
    .await
    .map_err(internal)?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(user_view(&s.pool, row).await?);
    }
    Ok(Json(out))
}

#[derive(Deserialize, ToSchema)]
#[schema(as = UsersCreateUserBody)]
struct CreateUserBody {
    email: String,
    display_name: String,
    password: String,
    #[serde(default)]
    role: Option<String>, // 'admin' | 'member' (default member)
    /// Doğrudan yayın yetkisi — verilmezse false (onay sürecine girer).
    #[serde(default)]
    can_publish: Option<bool>,
}

#[utoipa::path(post,
    operation_id = "users_create_user", path = "/", tag = "users",
    request_body = CreateUserBody,
    responses((status = 201, description = "Oluşturulan kullanıcı", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn create_user(
    State(s): State<AppState>,
    auth: AppAuth,
    Json(b): Json<CreateUserBody>,
) -> Result<(StatusCode, Json<UserView>), AppError> {
    auth.require_admin()?;
    let email = b.email.trim().to_lowercase();
    let name = b.display_name.trim();
    if email.is_empty() || name.is_empty() {
        return Err(AppError(
            "E-posta ve ad zorunludur".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    if b.password.len() < 6 {
        return Err(AppError(
            "Şifre en az 6 karakter olmalı".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let role = b.role.as_deref().unwrap_or("member");
    if !matches!(role, "admin" | "member") {
        return Err(AppError(
            "Rol 'admin' ya da 'member' olmalı".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let hash = bcrypt::hash(&b.password, BCRYPT_COST).map_err(internal)?;

    let row = sqlx::query_as::<_, UserRow>(&format!(
        "INSERT INTO wf.app_user (orgtnt_id, email, display_name, password_hash, role, can_publish) \
         VALUES ($1,$2,$3,$4,$5,$6) RETURNING {USER_COLS}",
    ))
    .bind(auth.orgtnt_id)
    .bind(&email)
    .bind(name)
    .bind(&hash)
    .bind(role)
    .bind(b.can_publish.unwrap_or(false))
    .fetch_one(&s.pool)
    .await
    .map_err(|e| match e.as_database_error().and_then(|d| d.constraint()) {
        Some("app_user_orgtnt_id_email_key") =>
            AppError(format!("{email}: bu e-posta ile kullanıcı zaten var"), StatusCode::CONFLICT),
        _ => internal(e),
    })?;
    let view = user_view(&s.pool, row).await?;
    Ok((StatusCode::CREATED, Json(view)))
}

#[derive(Deserialize, ToSchema)]
struct UpdateUserBody {
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    is_active: Option<bool>,
    /// Verilirse şifre sıfırlanır.
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    can_publish: Option<bool>,
}

#[utoipa::path(patch, path = "/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "Kullanıcı id")), request_body = UpdateUserBody,
    responses((status = 200, description = "Güncel kullanıcı", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn update_user(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(user_id): Path<Uuid>,
    Json(b): Json<UpdateUserBody>,
) -> Result<Json<UserView>, AppError> {
    auth.require_admin()?;
    let target = fetch_target(&s.pool, &auth, user_id).await?;

    if let Some(role) = b.role.as_deref() {
        if !matches!(role, "admin" | "member") {
            return Err(AppError(
                "Rol 'admin' ya da 'member' olmalı".into(),
                StatusCode::BAD_REQUEST,
            ));
        }
    }
    // Admin'i düşüren veya pasifleştiren değişiklikler son-admin kuralına takılır.
    let demotes = matches!(b.role.as_deref(), Some("member")) && target.role == "admin";
    let deactivates = b.is_active == Some(false) && target.is_active;
    if demotes || deactivates {
        guard_last_admin(&s.pool, &target).await?;
    }

    let password_hash = match b.password.as_deref() {
        Some(pw) if pw.len() < 6 => {
            return Err(AppError(
                "Şifre en az 6 karakter olmalı".into(),
                StatusCode::BAD_REQUEST,
            ))
        }
        Some(pw) => Some(bcrypt::hash(pw, BCRYPT_COST).map_err(internal)?),
        None => None,
    };
    let display_name = b
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());

    let row = sqlx::query_as::<_, UserRow>(&format!(
        "UPDATE wf.app_user SET \
             display_name = COALESCE($3, display_name), \
             role = COALESCE($4, role), \
             is_active = COALESCE($5, is_active), \
             password_hash = COALESCE($6, password_hash), \
             can_publish = COALESCE($7, can_publish), \
             updated_at = now() \
         WHERE user_id = $1 AND orgtnt_id = $2 RETURNING {USER_COLS}",
    ))
    .bind(user_id)
    .bind(auth.orgtnt_id)
    .bind(display_name)
    .bind(b.role.as_deref())
    .bind(b.is_active)
    .bind(password_hash.as_deref())
    .bind(b.can_publish)
    .fetch_one(&s.pool)
    .await
    .map_err(internal)?;
    user_view(&s.pool, row).await.map(Json)
}

#[utoipa::path(delete, path = "/{id}", tag = "users",
    params(("id" = Uuid, Path, description = "Kullanıcı id")),
    responses((status = 204, description = "Silindi")),
    security(("bearer_jwt" = [])))]
async fn delete_user(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(user_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    auth.require_admin()?;
    let target = fetch_target(&s.pool, &auth, user_id).await?;
    guard_last_admin(&s.pool, &target).await?;
    sqlx::query("DELETE FROM wf.app_user WHERE user_id = $1 AND orgtnt_id = $2")
        .bind(user_id)
        .bind(auth.orgtnt_id)
        .execute(&s.pool)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, ToSchema)]
struct AssignmentBody {
    project_id: Uuid,
    role: String, // 'admin' (project admin) | 'user' (tasarımcı)
}

/// Kullanıcının proje atamalarını topluca değiştirir (replace semantiği).
#[utoipa::path(put, path = "/{id}/projects", tag = "users",
    params(("id" = Uuid, Path, description = "Kullanıcı id")), request_body = Vec<AssignmentBody>,
    responses((status = 200, description = "Güncel proje üyelikleri", body = serde_json::Value)),
    security(("bearer_jwt" = [])))]
async fn set_projects(
    State(s): State<AppState>,
    auth: AppAuth,
    Path(user_id): Path<Uuid>,
    Json(assignments): Json<Vec<AssignmentBody>>,
) -> Result<Json<Vec<super::auth::ProjectMembership>>, AppError> {
    auth.require_admin()?;
    fetch_target(&s.pool, &auth, user_id).await?;

    for a in &assignments {
        if !matches!(a.role.as_str(), "admin" | "user") {
            return Err(AppError(
                "Proje rolü 'admin' ya da 'user' olmalı".into(),
                StatusCode::BAD_REQUEST,
            ));
        }
        wf_wfd::project::assert_in_tenant(&s.pool, a.project_id, auth.orgtnt_id)
            .await
            .map_err(|_| {
                AppError(
                    format!("Proje bulunamadı: {}", a.project_id),
                    StatusCode::NOT_FOUND,
                )
            })?;
    }

    let mut tx = s.pool.begin().await.map_err(internal)?;
    sqlx::query("DELETE FROM wf.project_member WHERE user_id = $1")
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    for a in &assignments {
        sqlx::query(
            "INSERT INTO wf.project_member (project_id, user_id, role) VALUES ($1,$2,$3) \
             ON CONFLICT (project_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(a.project_id)
        .bind(user_id)
        .bind(&a.role)
        .execute(&mut *tx)
        .await
        .map_err(internal)?;
    }
    tx.commit().await.map_err(internal)?;

    load_memberships(&s.pool, user_id).await.map(Json)
}
