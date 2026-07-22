use crate::error::AppError;
use axum::{
    extract::{Path, Query, State},
    routing::{get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;
use wf_org::{
    repo,
    traversal::{executor, parser},
};

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/orgtnt", get(list_orgtnt))
        .route("/orgtnt/:id", get(get_orgtnt))
        .route("/orgtnt/:id/orgt", get(list_orgt_by_tenant))
        .route(
            "/orgtnt/:id/users",
            get(list_users_by_tenant).post(create_user),
        )
        .route(
            "/orgtnt/:id/roles",
            get(list_roles_by_tenant).post(create_role),
        )
        .route("/orgtnt/:id/roles/:rid", patch(update_role))
        .route("/orgtnt/:id/actors", get(list_actors))
        .route(
            "/orgtnt/:id/assignments",
            post(create_assignment).delete(revoke_assignment),
        )
        .route(
            "/orgtnt/:id/orgu-roles",
            post(create_orgu_role).delete(delete_orgu_role),
        )
        .route(
            "/orgtnt/:id/delegations",
            get(list_delegations).post(create_delegation_admin),
        )
        .route(
            "/orgtnt/:id/delegations/:did",
            axum::routing::delete(revoke_delegation_admin),
        )
        .route("/orgt/:id/orgu", get(list_orgu_by_tree))
        .route("/users/:id/orgu", get(list_user_orgu))
        .route("/users/:id/roles", get(list_user_roles))
        .route("/orgu/:id", get(get_orgu))
        .route("/orgu/:id/traverse", get(traverse_orgu))
        .with_state(pool)
}

#[derive(Deserialize)]
struct PageQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

async fn list_orgtnt(
    State(pool): State<PgPool>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgtnt>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgtnt::list(&pool, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn get_orgtnt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgtnt>, AppError> {
    repo::orgtnt::get(&pool, id)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn list_orgt_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgt>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgt::list_by_tenant(&pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn list_users_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::User>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_users(&pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn list_roles_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Role>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_roles(&pool, orgtnt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Simülasyon aktör listesi: tenant'taki tüm (kullanıcı, birim, rol) atamaları.
/// Aktör switcher bundan beslenir — her satır bir X-Actor (orgu+user+role) demektir.
#[derive(Serialize, FromRow)]
struct ActorRow {
    user_id: Uuid,
    full_name: String,
    username: String,
    email: Option<String>,
    orgu_id: Uuid,
    orgu_name: String,
    role: String,
}

const ACTOR_SELECT: &str = "SELECT u.u_id AS user_id, u.full_name, u.username, u.email,
            o.orgu_id, o.name AS orgu_name, r.name AS role
     FROM org.ur ur
     JOIN org.u u  ON ur.u_id = u.u_id
     JOIN org.orgu o ON ur.orgu_id = o.orgu_id
     JOIN org.r r  ON ur.r_id = r.r_id
     WHERE ur.orgtnt_id = $1
       AND ur.ur_type <> 'excluded'
       AND u.is_active = true AND r.is_active = true";

async fn list_actors(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<ActorRow>>, AppError> {
    sqlx::query_as::<_, ActorRow>(&format!(
        "{ACTOR_SELECT} ORDER BY o.name, u.full_name, r.name"
    ))
    .bind(orgtnt_id)
    .fetch_all(&pool)
    .await
    .map(Json)
    .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))
}

/// Sim playground: yeni kullanıcı ekle.
#[derive(Deserialize)]
struct CreateUserBody {
    username: String,
    full_name: String,
    email: Option<String>,
}

async fn create_user(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateUserBody>,
) -> Result<Json<wf_org::models::User>, AppError> {
    repo::user_role::create_user(
        &pool,
        orgtnt_id,
        &body.username,
        &body.full_name,
        body.email.as_deref(),
    )
    .await
    .map(Json)
    .map_err(Into::into)
}

/// Yeni rol ekle veya aynı isimli pasif rolü yeniden aktifleştir.
#[derive(Deserialize)]
struct CreateRoleBody {
    name: String,
    display_name: Option<String>,
}

async fn create_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<CreateRoleBody>,
) -> Result<Json<wf_org::models::Role>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError(
            "rol adı boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::create_role(&pool, orgtnt_id, name, body.display_name.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Rol adını/görünen adını günceller. Kullanıcı atamaları r_id üzerinden kaldığı
/// için değişiklik rolün geçtiği tüm aktör listelerine yansır.
#[derive(Deserialize)]
struct UpdateRoleBody {
    name: String,
    display_name: String,
}

async fn update_role(
    State(pool): State<PgPool>,
    Path((orgtnt_id, r_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateRoleBody>,
) -> Result<Json<wf_org::models::Role>, AppError> {
    let name = body.name.trim();
    let display_name = body.display_name.trim();
    if name.is_empty() || display_name.is_empty() {
        return Err(AppError(
            "rol adı ve görünen ad boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::update_role(&pool, orgtnt_id, r_id, name, display_name)
        .await
        .map(Json)
        .map_err(Into::into)
}

/// Sim playground: (kullanıcı, birim, rol) atamasını garantiler → dönüşte hazır aktör satırı.
/// Kritere uygun aktör yoksa UI bununla bir aktör üretir.
#[derive(Deserialize)]
struct AssignBody {
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: String,
}

async fn create_assignment(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<AssignBody>,
) -> Result<Json<ActorRow>, AppError> {
    repo::user_role::grant_assignment(&pool, orgtnt_id, body.u_id, body.orgu_id, &body.role_name)
        .await
        .map_err(AppError::from)?;

    sqlx::query_as::<_, ActorRow>(&format!(
        "{ACTOR_SELECT} AND u.u_id = $2 AND o.orgu_id = $3 AND r.name = $4 LIMIT 1"
    ))
    .bind(orgtnt_id)
    .bind(body.u_id)
    .bind(body.orgu_id)
    .bind(&body.role_name)
    .fetch_one(&pool)
    .await
    .map(Json)
    .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))
}

/// Atama kaldırma: (u_id, orgu_id, role_name) query paramlarıyla granted rolü siler.
#[derive(Deserialize)]
struct RevokeAssignQuery {
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: String,
}

async fn revoke_assignment(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<RevokeAssignQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed =
        repo::user_role::revoke_assignment(&pool, orgtnt_id, q.u_id, q.orgu_id, &q.role_name)
            .await
            .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "atama bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}

/// Orgu'ya rol grant'ı: birimdeki tüm kullanıcılar bu rolü devralır (check_user_role).
#[derive(Deserialize)]
struct OrguRoleBody {
    orgu_id: Uuid,
    role_name: String,
}

async fn create_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<OrguRoleBody>,
) -> Result<axum::http::StatusCode, AppError> {
    let role_name = body.role_name.trim();
    if role_name.is_empty() {
        return Err(AppError(
            "rol adı boş olamaz".into(),
            axum::http::StatusCode::BAD_REQUEST,
        ));
    }
    repo::user_role::grant_orgu_role(&pool, orgtnt_id, body.orgu_id, role_name)
        .await
        .map_err(AppError::from)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// Orgu rol grant'ını kaldırma: (orgu_id, role_name) query paramlarıyla.
#[derive(Deserialize)]
struct RevokeOrguRoleQuery {
    orgu_id: Uuid,
    role_name: String,
}

async fn delete_orgu_role(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(q): Query<RevokeOrguRoleQuery>,
) -> Result<axum::http::StatusCode, AppError> {
    let removed = repo::user_role::revoke_orgu_role(&pool, orgtnt_id, q.orgu_id, &q.role_name)
        .await
        .map_err(AppError::from)?;
    if removed {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "birim-rol bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}

async fn list_orgu_by_tree(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgu::list_by_tree(&pool, orgt_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn list_user_orgu(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserOrgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_orgus(&pool, user_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn list_user_roles(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserRole>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_roles(&pool, user_id, limit, offset)
        .await
        .map(Json)
        .map_err(Into::into)
}

async fn get_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgu>, AppError> {
    repo::orgu::get(&pool, orgu_id)
        .await
        .map(Json)
        .map_err(Into::into)
}

#[derive(Deserialize)]
struct TraverseQuery {
    expr: String,
}

async fn traverse_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Query(q): Query<TraverseQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let orgt_id = repo::orgu::get_orgt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;
    let orgtnt_id = repo::orgu::get_orgtnt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;

    let expr = normalize_traverse_expr(&q.expr);
    let pipeline = parser::parse(&expr)
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::BAD_REQUEST))?;

    let result = executor::execute(&pool, orgu_id, orgt_id, orgtnt_id, &pipeline)
        .await
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(result))
}

fn normalize_traverse_expr(expr: &str) -> String {
    let expr = expr.trim();
    // Global tip selektörü (*:[...]) anchor'dan bağımsızdır — "self." ile sarma.
    if expr == "self" || expr.starts_with("self.") || expr.starts_with("*:") {
        expr.to_string()
    } else {
        format!("self.{expr}")
    }
}

// ── Madde 6: vekalet/delegasyon (admin yönetim; /org X-Admin-Key kapısı altında) ──

async fn list_delegations(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    repo::delegation::list_by_tenant(&pool, orgtnt_id)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize)]
struct SeatBody {
    orgu_id: Uuid,
    role: String,
}

#[derive(Deserialize)]
struct AdminDelegationBody {
    delegator_user_id: Uuid,
    #[serde(default)]
    seat: Option<SeatBody>,
    #[serde(default)]
    all: bool,
    grantee: serde_json::Value,
    #[serde(default)]
    valid_from: Option<chrono::DateTime<chrono::Utc>>,
    valid_to: chrono::DateTime<chrono::Utc>,
}

async fn create_delegation_admin(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<AdminDelegationBody>,
) -> Result<Json<Vec<wf_org::models::Delegation>>, AppError> {
    let seat = body.seat.map(|s| (s.orgu_id, s.role));
    let created = crate::routes::delegation::create_delegations(
        &pool,
        orgtnt_id,
        body.delegator_user_id,
        seat,
        body.all,
        &body.grantee,
        body.valid_from,
        body.valid_to,
        body.delegator_user_id, // admin yolunda created_by = delegator
    )
    .await?;
    Ok(Json(created))
}

async fn revoke_delegation_admin(
    State(pool): State<PgPool>,
    Path((orgtnt_id, did)): Path<(Uuid, Uuid)>,
) -> Result<axum::http::StatusCode, AppError> {
    let ok = repo::delegation::revoke(&pool, did, orgtnt_id)
        .await
        .map_err(AppError::from)?;
    if ok {
        Ok(axum::http::StatusCode::NO_CONTENT)
    } else {
        Err(AppError(
            "vekalet bulunamadı".into(),
            axum::http::StatusCode::NOT_FOUND,
        ))
    }
}
