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
        .route("/orgt/:id/orgu",     get(list_orgu_by_tree))
        .route("/orgu/:id",          get(get_orgu))
        .route("/orgu/:id/traverse", get(traverse_orgu))
        .with_state(pool)
}

async fn list_orgtnt(
    State(pool): State<PgPool>,
) -> Result<Json<Vec<wf_org::models::Orgtnt>>, AppError> {
    repo::orgtnt::list(&pool).await.map(Json).map_err(Into::into)
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
) -> Result<Json<Vec<wf_org::models::Orgt>>, AppError> {
    repo::orgt::list_by_tenant(&pool, orgtnt_id).await.map(Json).map_err(Into::into)
}

async fn list_orgu_by_tree(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::models::Orgu>>, AppError> {
    repo::orgu::list_by_tree(&pool, orgt_id).await.map(Json).map_err(Into::into)
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

    let pipeline = parser::parse(&q.expr)
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::BAD_REQUEST))?;

    let result = executor::execute(&pool, orgu_id, orgt_id, &pipeline)
        .await
        .map_err(|e| AppError(e.to_string(), axum::http::StatusCode::INTERNAL_SERVER_ERROR))?;

    Ok(Json(result))
}
