use axum::{
    extract::{Path, Query, State},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use wf_org::{repo, traversal::{executor, parser}};
use crate::error::AppError;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/orgtnt",            get(list_orgtnt))
        .route("/orgtnt/:id",        get(get_orgtnt))
        .route("/orgtnt/:id/orgt",   get(list_orgt_by_tenant))
        .route("/orgtnt/:id/users",  get(list_users_by_tenant))
        .route("/orgtnt/:id/roles",  get(list_roles_by_tenant))
        .route("/orgt/:id/orgu",     get(list_orgu_by_tree))
        .route("/users/:id/orgu",    get(list_user_orgu))
        .route("/users/:id/roles",   get(list_user_roles))
        .route("/orgu/:id",          get(get_orgu))
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
    repo::orgtnt::list(&pool, limit, offset).await.map(Json).map_err(Into::into)
}

async fn get_orgtnt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgtnt>, AppError> {
    repo::orgtnt::get(&pool, id).await.map(Json).map_err(Into::into)
}

async fn list_orgt_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgt>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgt::list_by_tenant(&pool, orgtnt_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn list_users_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::User>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_users(&pool, orgtnt_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn list_roles_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Role>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_roles(&pool, orgtnt_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn list_orgu_by_tree(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::orgu::list_by_tree(&pool, orgt_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn list_user_orgu(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserOrgu>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_orgus(&pool, user_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn list_user_roles(
    State(pool): State<PgPool>,
    Path(user_id): Path<Uuid>,
    Query(page): Query<PageQuery>,
) -> Result<Json<Vec<wf_org::models::UserRole>>, AppError> {
    let limit = page.limit.unwrap_or(50).clamp(1, 200);
    let offset = page.offset.unwrap_or(0).max(0);
    repo::user_role::list_user_roles(&pool, user_id, limit, offset).await.map(Json).map_err(Into::into)
}

async fn get_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgu>, AppError> {
    repo::orgu::get(&pool, orgu_id).await.map(Json).map_err(Into::into)
}

#[derive(Deserialize)]
struct TraverseQuery { expr: String }

async fn traverse_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Query(q): Query<TraverseQuery>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    let orgt_id = repo::orgu::get_orgt_id(&pool, orgu_id)
        .await
        .map_err(AppError::from)?;

    let expr = normalize_traverse_expr(&q.expr);
    let pipeline = parser::parse(&expr)
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::BAD_REQUEST))?;

    let result = executor::execute(&pool, orgu_id, orgt_id, &pipeline)
        .await
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(result))
}

fn normalize_traverse_expr(expr: &str) -> String {
    let expr = expr.trim();
    if expr == "self" || expr.starts_with("self.") {
        expr.to_string()
    } else {
        format!("self.{expr}")
    }
}
