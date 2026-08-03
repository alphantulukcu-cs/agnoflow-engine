//! Seçilen bağlantıya karşı sorgu çalıştırma (Faz 2). Postgres/MySQL/SQLite sqlx,
//! MSSQL tiberius. Param `:name` → sürücüye özel işaret; satırlar JSON'a map edilir.
use super::{
    bind_params, mysql_connect_options, sqlite_uri, sqlx_uri, DbConfig, DbDriver, DbError,
};
use rust_decimal::prelude::ToPrimitive;
use serde_json::{json, Map, Number, Value};
use sqlx::{Column, Row, TypeInfo};

pub enum RunHandle {
    Pg(sqlx::PgPool),
    My(sqlx::MySqlPool),
    Ms(tiberius::Config),
    Lite(sqlx::SqlitePool),
}

pub async fn connect(cfg: &DbConfig) -> Result<RunHandle, DbError> {
    match cfg.driver {
        DbDriver::Postgres => {
            let pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlx_uri(cfg, "postgres", 5432))
                .await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::Pg(pool))
        }
        DbDriver::Mysql => {
            let pool = sqlx::mysql::MySqlPoolOptions::new()
                .max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect_with(mysql_connect_options(cfg)?)
                .await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::My(pool))
        }
        DbDriver::Sqlite => {
            let pool = sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(2)
                .acquire_timeout(std::time::Duration::from_secs(8))
                .connect(&sqlite_uri(cfg))
                .await
                .map_err(|e| DbError(e.to_string()))?;
            Ok(RunHandle::Lite(pool))
        }
        DbDriver::Mssql => {
            use tiberius::{AuthMethod, Config};
            let mut c = Config::new();
            if cfg.mode == "uri" {
                c = Config::from_ado_string(cfg.secret.as_deref().unwrap_or(""))
                    .map_err(|e| DbError(e.to_string()))?;
            } else {
                c.host(cfg.host.as_deref().unwrap_or("localhost"));
                c.port(cfg.port.unwrap_or(1433) as u16);
                if let Some(db) = &cfg.database {
                    c.database(db);
                }
                c.authentication(AuthMethod::sql_server(
                    cfg.username.as_deref().unwrap_or(""),
                    cfg.secret.as_deref().unwrap_or(""),
                ));
                if cfg.options.get("encrypt").and_then(|v| v.as_str()) == Some("false") {
                    c.encryption(tiberius::EncryptionLevel::NotSupported);
                } else {
                    c.trust_cert();
                }
            }
            Ok(RunHandle::Ms(c))
        }
    }
}

/// SQL'i çalıştırır, satırları JSON'a map eder. Tek satır → obje, çoklu → {rows:[...]}.
pub async fn run_query(
    handle: &RunHandle,
    query: &str,
    params: &Map<String, Value>,
) -> Result<Value, DbError> {
    let rows = match handle {
        RunHandle::Pg(pool) => {
            let (sql, vals) = bind_params(query, params, |i| format!("${i}"));
            let mut q = sqlx::query(&sql);
            for v in &vals {
                q = bind_pg(q, v);
            }
            let rows = q
                .fetch_all(pool)
                .await
                .map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(pg_row_json).collect::<Vec<_>>()
        }
        RunHandle::My(pool) => {
            let (sql, vals) = bind_params(query, params, |_| "?".to_string());
            let mut q = sqlx::query(&sql);
            for v in &vals {
                q = bind_my(q, v);
            }
            let rows = q
                .fetch_all(pool)
                .await
                .map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(my_row_json).collect::<Vec<_>>()
        }
        RunHandle::Lite(pool) => {
            let (sql, vals) = bind_params(query, params, |_| "?".to_string());
            let mut q = sqlx::query(&sql);
            for v in &vals {
                q = bind_lite(q, v);
            }
            let rows = q
                .fetch_all(pool)
                .await
                .map_err(|e| DbError(e.to_string()))?;
            rows.iter().map(lite_row_json).collect::<Vec<_>>()
        }
        RunHandle::Ms(config) => {
            use tokio::net::TcpStream;
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let (sql, vals) = bind_params(query, params, |i| format!("@P{i}"));
            let tcp = TcpStream::connect(config.get_addr())
                .await
                .map_err(|e| DbError(e.to_string()))?;
            tcp.set_nodelay(true).ok();
            let mut client = tiberius::Client::connect(config.clone(), tcp.compat_write())
                .await
                .map_err(|e| DbError(e.to_string()))?;
            let mut q = tiberius::Query::new(sql);
            for v in &vals {
                push_ms(&mut q, v);
            }
            let stream = q
                .query(&mut client)
                .await
                .map_err(|e| DbError(e.to_string()))?;
            let ms_rows = stream
                .into_first_result()
                .await
                .map_err(|e| DbError(e.to_string()))?;
            ms_rows.iter().map(ms_row_json).collect::<Vec<_>>()
        }
    };
    Ok(match rows.len() {
        1 => rows.into_iter().next().unwrap(),
        _ => json!({ "rows": rows }),
    })
}

fn decimal_json(d: rust_decimal::Decimal) -> Value {
    // f64'e sığıyorsa sayı, değilse hassasiyet kaybetmemek için string
    d.to_f64()
        .and_then(Number::from_f64)
        .map(Value::Number)
        .unwrap_or_else(|| Value::from(d.to_string()))
}

// ── sqlx bind yardımcıları ──
fn bind_pg<'a>(
    q: sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments>,
    v: &Value,
) -> sqlx::query::Query<'a, sqlx::Postgres, sqlx::postgres::PgArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        Value::Null => q.bind(Option::<String>::None),
        other => q.bind(other.clone()), // obje/dizi → jsonb
    }
}
fn bind_my<'a>(
    q: sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    v: &Value,
) -> sqlx::query::Query<'a, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        Value::Null => q.bind(Option::<String>::None),
        other => q.bind(other.clone()),
    }
}
fn bind_lite<'a>(
    q: sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>>,
    v: &Value,
) -> sqlx::query::Query<'a, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'a>> {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
        Value::Number(n) => q.bind(n.as_f64()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        Value::Null => q.bind(Option::<String>::None),
        other => q.bind(other.to_string()),
    }
}
fn push_ms(q: &mut tiberius::Query, v: &Value) {
    match v {
        Value::Number(n) if n.is_i64() => q.bind(n.as_i64().unwrap()),
        Value::Number(n) => q.bind(n.as_f64().unwrap()),
        Value::Bool(b) => q.bind(*b),
        Value::String(s) => q.bind(s.clone()),
        Value::Null => q.bind(Option::<&str>::None),
        other => q.bind(other.to_string()),
    }
}

// ── satır → JSON ──
// Sıra önemli değil (try_get tip uyuşmazlığında Err döner); kapsam önemli.
fn pg_row_json(row: &sqlx::postgres::PgRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let n = col.name();
        let val: Value = row
            .try_get::<Value, _>(n) // json/jsonb
            .or_else(|_| row.try_get::<bool, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<i16, _>(n).map(Value::from)) // int2
            .or_else(|_| row.try_get::<i32, _>(n).map(Value::from)) // int4
            .or_else(|_| row.try_get::<i64, _>(n).map(Value::from)) // int8
            .or_else(|_| row.try_get::<f32, _>(n).map(|f| Value::from(f as f64)))
            .or_else(|_| row.try_get::<f64, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<rust_decimal::Decimal, _>(n).map(decimal_json)) // numeric
            .or_else(|_| {
                row.try_get::<uuid::Uuid, _>(n)
                    .map(|u| Value::from(u.to_string()))
            })
            .or_else(|_| {
                row.try_get::<chrono::DateTime<chrono::Utc>, _>(n)
                    .map(|t| Value::from(t.to_rfc3339()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveDateTime, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveDate, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveTime, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| row.try_get::<String, _>(n).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(n.to_string(), val);
    }
    Value::Object(obj)
}
fn my_row_json(row: &sqlx::mysql::MySqlRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let n = col.name();
        // DİKKAT: sqlx-mysql bool decode'u TÜM int tiplerini kabul eder (0/1'e indirger)
        // — bool bu yüzden sayısal zincirin SONUNDA denenir (TINYINT(1) zaten i8 yakalar).
        let val: Value = row
            .try_get::<Value, _>(n)
            .or_else(|_| row.try_get::<i8, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<i16, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<i32, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<i64, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<u64, _>(n).map(Value::from)) // BIGINT UNSIGNED
            .or_else(|_| row.try_get::<f32, _>(n).map(|f| Value::from(f as f64)))
            .or_else(|_| row.try_get::<f64, _>(n).map(Value::from))
            .or_else(|_| row.try_get::<rust_decimal::Decimal, _>(n).map(decimal_json))
            .or_else(|_| row.try_get::<bool, _>(n).map(Value::from))
            .or_else(|_| {
                row.try_get::<chrono::DateTime<chrono::Utc>, _>(n)
                    .map(|t| Value::from(t.to_rfc3339()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveDateTime, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveDate, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| {
                row.try_get::<chrono::NaiveTime, _>(n)
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|_| row.try_get::<String, _>(n).map(Value::from))
            .unwrap_or(Value::Null);
        obj.insert(n.to_string(), val);
    }
    Value::Object(obj)
}
fn lite_row_json(row: &sqlx::sqlite::SqliteRow) -> Value {
    let mut obj = Map::new();
    for col in row.columns() {
        let n = col.name();
        // SQLite dinamik tiplidir; NULL non-Option tiplere 0/"" decode edilir —
        // bu yüzden değerin GERÇEK tipine bak (INTEGER/REAL/TEXT/BLOB/NULL).
        use sqlx::ValueRef;
        let val: Value = match row.try_get_raw(n) {
            Err(_) => Value::Null,
            Ok(raw) => match raw.type_info().name() {
                "INTEGER" => row
                    .try_get::<i64, _>(n)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "REAL" => row
                    .try_get::<f64, _>(n)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "TEXT" => row
                    .try_get::<String, _>(n)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                _ => Value::Null, // NULL ve BLOB
            },
        };
        obj.insert(n.to_string(), val);
    }
    Value::Object(obj)
}
#[cfg(test)]
mod sqlite_tests {
    use super::*;

    async fn mem(name: &str) -> RunHandle {
        // shared-cache in-memory: havuzdaki tüm bağlantılar aynı DB'yi görür
        let cfg = DbConfig {
            driver: DbDriver::Sqlite,
            mode: "uri".into(),
            host: None,
            port: None,
            database: None,
            username: None,
            secret: Some(format!("sqlite:file:{name}?mode=memory&cache=shared")),
            options: json!({}),
        };
        connect(&cfg).await.unwrap()
    }

    #[tokio::test]
    async fn sqlite_roundtrip_types_and_params() {
        let h = mem("t1").await;
        let empty = Map::new();
        run_query(
            &h,
            "CREATE TABLE t (id INTEGER, name TEXT, score REAL, ok BOOLEAN)",
            &empty,
        )
        .await
        .unwrap();
        let mut p = Map::new();
        p.insert("id".into(), json!(7));
        p.insert("name".into(), json!("ayşe"));
        p.insert("score".into(), json!(3.5));
        p.insert("ok".into(), json!(true));
        run_query(&h, "INSERT INTO t VALUES (:id, :name, :score, :ok)", &p)
            .await
            .unwrap();

        let mut q = Map::new();
        q.insert("id".into(), json!(7));
        let row = run_query(&h, "SELECT * FROM t WHERE id = :id", &q)
            .await
            .unwrap();
        assert_eq!(row["id"], json!(7));
        assert_eq!(row["name"], json!("ayşe"));
        assert_eq!(row["score"], json!(3.5));
        assert_eq!(row["ok"], json!(1)); // sqlite bool → integer

        run_query(&h, "INSERT INTO t VALUES (8, 'ali', 1.0, 0)", &empty)
            .await
            .unwrap();
        let multi = run_query(&h, "SELECT id FROM t ORDER BY id", &empty)
            .await
            .unwrap();
        assert_eq!(multi["rows"], json!([{"id": 7}, {"id": 8}]));
    }

    #[tokio::test]
    async fn sqlite_null_and_empty() {
        let h = mem("t2").await;
        let empty = Map::new();
        // not: NOTHING sqlite'ta rezerve kelimedir — alias olarak kullanma
        let row = run_query(&h, "SELECT NULL AS bos, 42 AS n", &empty)
            .await
            .unwrap();
        assert_eq!(row["bos"], Value::Null);
        assert_eq!(row["n"], json!(42));
        let none = run_query(&h, "SELECT 1 WHERE 0", &empty).await.unwrap();
        assert_eq!(none, json!({"rows": []}));
    }
}

fn ms_row_json(row: &tiberius::Row) -> Value {
    let mut obj = Map::new();
    let cols: Vec<String> = row.columns().iter().map(|c| c.name().to_string()).collect();
    for (i, name) in cols.iter().enumerate() {
        let val: Value = row
            .try_get::<&str, _>(i)
            .ok()
            .flatten()
            .map(|s| Value::from(s.to_string()))
            .or_else(|| row.try_get::<bool, _>(i).ok().flatten().map(Value::from))
            .or_else(|| {
                row.try_get::<u8, _>(i)
                    .ok()
                    .flatten()
                    .map(|n| Value::from(n as i64))
            }) // tinyint
            .or_else(|| {
                row.try_get::<i16, _>(i)
                    .ok()
                    .flatten()
                    .map(|n| Value::from(n as i64))
            })
            .or_else(|| {
                row.try_get::<i32, _>(i)
                    .ok()
                    .flatten()
                    .map(|n| Value::from(n as i64))
            })
            .or_else(|| row.try_get::<i64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| {
                row.try_get::<f32, _>(i)
                    .ok()
                    .flatten()
                    .map(|f| Value::from(f as f64))
            })
            .or_else(|| row.try_get::<f64, _>(i).ok().flatten().map(Value::from))
            .or_else(|| {
                row.try_get::<rust_decimal::Decimal, _>(i)
                    .ok()
                    .flatten()
                    .map(decimal_json)
            })
            .or_else(|| {
                row.try_get::<uuid::Uuid, _>(i)
                    .ok()
                    .flatten()
                    .map(|u| Value::from(u.to_string()))
            })
            .or_else(|| {
                row.try_get::<chrono::DateTime<chrono::Utc>, _>(i)
                    .ok()
                    .flatten()
                    .map(|t| Value::from(t.to_rfc3339()))
            })
            .or_else(|| {
                row.try_get::<chrono::NaiveDateTime, _>(i)
                    .ok()
                    .flatten()
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|| {
                row.try_get::<chrono::NaiveDate, _>(i)
                    .ok()
                    .flatten()
                    .map(|t| Value::from(t.to_string()))
            })
            .or_else(|| {
                row.try_get::<chrono::NaiveTime, _>(i)
                    .ok()
                    .flatten()
                    .map(|t| Value::from(t.to_string()))
            })
            .unwrap_or(Value::Null);
        obj.insert(name.clone(), val);
    }
    Value::Object(obj)
}
