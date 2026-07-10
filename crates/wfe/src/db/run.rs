//! Seçilen bağlantıya karşı sorgu çalıştırma (Faz 2). Postgres/MySQL sqlx,
//! MSSQL tiberius. Param `:name` → sürücüye özel işaret; satırlar JSON'a map edilir.
use super::{bind_params, DbConfig, DbDriver, DbError};
use serde_json::{json, Map, Value};
use sqlx::{Column, Row};

pub enum RunHandle {
    Pg(sqlx::PgPool),
    My(sqlx::MySqlPool),
    Ms(tiberius::Config),
}

fn sqlx_uri(cfg: &DbConfig, scheme: &str, default_port: i32) -> String {
    if cfg.mode == "uri" { return cfg.secret.clone().unwrap_or_default(); }
    let host = cfg.host.as_deref().unwrap_or("localhost");
    let port = cfg.port.unwrap_or(default_port);
    let db = cfg.database.as_deref().unwrap_or("");
    let user = cfg.username.as_deref().unwrap_or("");
    let pass = cfg.secret.as_deref().unwrap_or("");
    format!("{scheme}://{user}:{pass}@{host}:{port}/{db}")
}

pub async fn connect(cfg: &DbConfig) -> Result<RunHandle, DbError> {
    match cfg.driver {
        DbDriver::Postgres => {
            let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlx_uri(cfg, "postgres", 5432)).await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::Pg(pool))
        }
        DbDriver::Mysql => {
            let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlx_uri(cfg, "mysql", 3306)).await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::My(pool))
        }
        DbDriver::Mssql => {
            use tiberius::{Config, AuthMethod};
            let mut c = Config::new();
            if cfg.mode == "uri" {
                c = Config::from_ado_string(cfg.secret.as_deref().unwrap_or(""))
                    .map_err(|e| DbError(e.to_string()))?;
            } else {
                c.host(cfg.host.as_deref().unwrap_or("localhost"));
                c.port(cfg.port.unwrap_or(1433) as u16);
                if let Some(db) = &cfg.database { c.database(db); }
                c.authentication(AuthMethod::sql_server(
                    cfg.username.as_deref().unwrap_or(""),
                    cfg.secret.as_deref().unwrap_or("")));
                if cfg.options.get("encrypt").and_then(|v| v.as_str()) == Some("false") {
                    c.encryption(tiberius::EncryptionLevel::NotSupported);
                } else { c.trust_cert(); }
            }
            Ok(RunHandle::Ms(c))
        }
    }
}

/// SQL'i çalıştırır, satırları JSON'a map eder. Tek satır → obje, çoklu → {rows:[...]}.
pub async fn run_query(handle: &RunHandle, query: &str, params: &Map<String, Value>) -> Result<Value, DbError> {
    let rows = match handle {
        RunHandle::Pg(pool) => {
            let (sql, vals) = bind_params(query, params, |i| format!("${i}"));
            let mut q = sqlx::query(&sql);
            for v in &vals { q = bind_pg(q, v); }
            let rows = q.fetch_all(pool).await.map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(pg_row_json).collect::<Vec<_>>()
        }
        RunHandle::My(pool) => {
            let (sql, vals) = bind_params(query, params, |_| "?".to_string());
            let mut q = sqlx::query(&sql);
            for v in &vals { q = bind_my(q, v); }
            let rows = q.fetch_all(pool).await.map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(my_row_json).collect::<Vec<_>>()
        }
        RunHandle::Ms(config) => {
            use tokio::net::TcpStream;
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let (sql, vals) = bind_params(query, params, |i| format!("@P{i}"));
            let tcp = TcpStream::connect(config.get_addr()).await.map_err(|e| DbError(e.to_string()))?;
            tcp.set_nodelay(true).ok();
            let mut client = tiberius::Client::connect(config.clone(), tcp.compat_write()).await
                .map_err(|e| DbError(e.to_string()))?;
            let mut q = tiberius::Query::new(sql);
            for v in &vals { push_ms(&mut q, v); }
            let stream = q.query(&mut client).await.map_err(|e| DbError(e.to_string()))?;
            let ms_rows = stream.into_first_result().await.map_err(|e| DbError(e.to_string()))?;
            ms_rows.iter().map(ms_row_json).collect::<Vec<_>>()
        }
    };
    Ok(match rows.len() { 1 => rows.into_iter().next().unwrap(), _ => json!({ "rows": rows }) })
}

// ── sqlx bind yardımcıları ──
fn bind_pg<'a>(q: sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments>, v: &Value)
    -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}
fn bind_my<'a>(q: sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments>, v: &Value)
    -> sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}
fn push_ms(q: &mut tiberius::Query, v: &Value) {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64().unwrap()),
        Value::Number(n) => q.bind(n.as_f64().unwrap()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        other => q.bind(other.to_string()),
    }
}

// ── satır → JSON ──
fn pg_row_json(row: &sqlx::postgres::PgRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let val: Value = row.try_get::<Value, _>(col.name())
            .or_else(|_| row.try_get::<String, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<i64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<f64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<bool, _>(col.name()).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(col.name().to_string(), val);
    }
    Value::Object(obj)
}
fn my_row_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let val: Value = row.try_get::<String, _>(col.name()).map(Value::from)
            .or_else(|_| row.try_get::<i64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<f64, _>(col.name()).map(Value::from))
            .or_else(|_| row.try_get::<bool, _>(col.name()).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(col.name().to_string(), val);
    }
    Value::Object(obj)
}
fn ms_row_json(row: &tiberius::Row) -> Value {
    let mut obj = Map::new();
    let cols: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    for (i, name) in cols.iter().enumerate() {
        let val: Value = row.try_get::<&str, _>(i).ok().flatten().map(|s| Value::from(s.to_string()))
            .or_else(|| row.try_get::<i32, _>(i).ok().flatten().map(|n| Value::from(n as i64)))
            .or_else(|| row.try_get::<i64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| row.try_get::<f64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| row.try_get::<bool, _>(i).ok().flatten().map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(name.clone(), val);
    }
    Value::Object(obj)
}
