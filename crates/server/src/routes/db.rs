use crate::{error::AppError, state::AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::db::{self, crypto, DbConfig, DbDriver};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/connections", get(list).post(create))
        .route("/connections/test", post(test_draft))
        .route(
            "/connections/:id",
            axum::routing::put(update).delete(delete),
        )
        .route("/connections/:id/test", post(test_saved))
        .with_state(state)
}

#[derive(Deserialize)]
struct TenantQuery {
    orgtnt_id: Uuid,
}

#[derive(Deserialize)]
struct ConnBody {
    orgtnt_id: Option<Uuid>,
    name: Option<String>,
    driver: String,
    #[serde(default = "default_mode")]
    mode: String,
    host: Option<String>,
    port: Option<i32>,
    database: Option<String>,
    username: Option<String>,
    #[serde(default)]
    options: Value,
    /// Parola/dizedeki gizli — verilmezse (update) mevcut korunur.
    secret: Option<String>,
}
fn default_mode() -> String {
    "fields".into()
}

fn to_config(b: &ConnBody, secret: Option<String>) -> Result<DbConfig, AppError> {
    let driver = DbDriver::parse(&b.driver)
        .ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::BAD_REQUEST))?;
    Ok(DbConfig {
        driver,
        mode: b.mode.clone(),
        host: b.host.clone(),
        port: b.port,
        database: b.database.clone(),
        username: b.username.clone(),
        secret,
        options: if b.options.is_null() {
            json!({})
        } else {
            b.options.clone()
        },
    })
}

async fn list(
    State(s): State<AppState>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Value>, AppError> {
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>, Option<i32>, Option<String>, Option<String>, Value, bool, Option<bool>, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT id, name, driver, mode, host, port, database, username, options, is_active, last_test_ok, last_test_at \
         FROM wf.db_connection WHERE orgtnt_id=$1 AND is_active=true ORDER BY name")
        .bind(q.orgtnt_id).fetch_all(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    // secret ASLA dönmez
    let items: Vec<Value> = rows
        .into_iter()
        .map(|r| {
            json!({
                "id": r.0, "name": r.1, "driver": r.2, "mode": r.3, "host": r.4, "port": r.5,
                "database": r.6, "username": r.7, "options": r.8, "is_active": r.9,
                "last_test_ok": r.10, "last_test_at": r.11,
            })
        })
        .collect();
    Ok(Json(json!(items)))
}

async fn create(
    State(s): State<AppState>,
    Json(b): Json<ConnBody>,
) -> Result<Json<Value>, AppError> {
    let orgtnt = b
        .orgtnt_id
        .ok_or_else(|| AppError("orgtnt_id gerekli".into(), StatusCode::BAD_REQUEST))?;
    let name = b
        .name
        .clone()
        .ok_or_else(|| AppError("name gerekli".into(), StatusCode::BAD_REQUEST))?;
    let enc = match &b.secret {
        Some(sec) => Some(
            crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?,
        ),
        None => None,
    };
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.db_connection (orgtnt_id,name,driver,mode,host,port,database,username,options,secret_enc) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) RETURNING id")
        .bind(orgtnt).bind(&name).bind(&b.driver).bind(&b.mode)
        .bind(&b.host).bind(b.port).bind(&b.database).bind(&b.username)
        .bind(&b.options).bind(enc)
        .fetch_one(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;
    Ok(Json(json!({ "id": id })))
}

async fn update(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
    Json(b): Json<ConnBody>,
) -> Result<StatusCode, AppError> {
    // secret verilmezse mevcut korunur (COALESCE): None → NULL bind → COALESCE(NULL, secret_enc)
    let enc: Option<Vec<u8>> = match &b.secret {
        Some(sec) => Some(
            crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?,
        ),
        None => None,
    };
    let n = sqlx::query(
        "UPDATE wf.db_connection SET name=$2, driver=$3, mode=$4, host=$5, port=$6, database=$7, \
         username=$8, options=$9, secret_enc=COALESCE($10, secret_enc), updated_at=now() WHERE id=$1")
        .bind(id).bind(&b.name).bind(&b.driver).bind(&b.mode).bind(&b.host).bind(b.port)
        .bind(&b.database).bind(&b.username).bind(&b.options).bind(enc)
        .execute(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?.rows_affected();
    if n == 0 {
        return Err(AppError(
            "bağlantı bulunamadı".into(),
            StatusCode::NOT_FOUND,
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM wf.db_connection WHERE id=$1")
        .bind(id)
        .execute(&s.pool)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_draft(
    State(_s): State<AppState>,
    Json(b): Json<ConnBody>,
) -> Result<Json<Value>, AppError> {
    let cfg = to_config(&b, b.secret.clone())?;
    Ok(Json(run_test(&cfg).await))
}

async fn test_saved(
    State(s): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            Option<i32>,
            Option<String>,
            Option<String>,
            Value,
            Option<Vec<u8>>,
        ),
    >(
        "SELECT driver, mode, host, port, database, username, options, secret_enc \
         FROM wf.db_connection WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
    .ok_or_else(|| AppError("bağlantı bulunamadı".into(), StatusCode::NOT_FOUND))?;
    let driver = DbDriver::parse(&row.0)
        .ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let secret = match row.7 {
        Some(bytes) => Some(
            crypto::decrypt(&bytes)
                .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?,
        ),
        None => None,
    };
    let cfg = DbConfig {
        driver,
        mode: row.1,
        host: row.2,
        port: row.3,
        database: row.4,
        username: row.5,
        secret,
        options: row.6,
    };
    let result = run_test(&cfg).await;
    let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let _ =
        sqlx::query("UPDATE wf.db_connection SET last_test_at=now(), last_test_ok=$2 WHERE id=$1")
            .bind(id)
            .bind(ok)
            .execute(&s.pool)
            .await;
    Ok(Json(result))
}

async fn run_test(cfg: &DbConfig) -> Value {
    match db::drivers::test(cfg).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "message": e.to_string() }),
    }
}
