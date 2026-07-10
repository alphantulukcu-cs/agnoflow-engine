//! v2.2 AutoexecRunner — REST / SQL / CALC (WOR-41).
//! Timeout PIPELINE tarafından uygulanır (M5); burada yalnızca ham çalıştırma var.
//! Config içindeki $-string parametreleri ExecEnv ctx'i ile çözülür.

use async_trait::async_trait;
use reqwest::{Client, Method};
use serde_json::{json, Map, Value};
use sqlx::{Column, PgPool, Row};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;
use wfe_core::types::wfd_v22::{AutoexecDef, AutoexecType};
use wfe_core::v22::eval::{evaluate_value, EvalEnv};
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure};

use crate::db::run::RunHandle;
use crate::db::{self, DbConfig, DbDriver};

pub struct LiveAutoexecRunner {
    client: Client,
    pool: Option<PgPool>,
    // (connection_id, updated_at_epoch) → çalıştırılabilir handle önbelleği
    registry: Mutex<HashMap<Uuid, (i64, Arc<RunHandle>)>>,
}

impl LiveAutoexecRunner {
    pub fn new(pool: Option<PgPool>) -> Self {
        Self {
            client: Client::new(),
            pool,
            registry: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl AutoexecRunner for LiveAutoexecRunner {
    async fn run(&self, def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
        match def.kind {
            AutoexecType::Rest => self.run_rest(def, env).await,
            AutoexecType::Calc => run_calc(def, env),
            AutoexecType::Sql => self.run_sql(def, env).await,
            AutoexecType::Python | AutoexecType::Lambda => Err(ExecFailure::failed(
                "python/lambda autoexec tipleri henüz desteklenmiyor",
            )),
        }
    }
}

impl LiveAutoexecRunner {
    async fn run_rest(&self, def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
        let method = def.config.get("method").and_then(Value::as_str).unwrap_or("GET");
        let method = Method::from_str(method)
            .map_err(|_| ExecFailure::failed(format!("geçersiz HTTP metodu: {method}")))?;
        let url = def
            .config
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ExecFailure::failed("rest config'te url yok"))?;

        let params = def
            .config
            .get("params")
            .map(|p| resolve_config_value(p, env))
            .unwrap_or(Value::Null);
        let body = def
            .config
            .get("body")
            .map(|b| resolve_config_value(b, env));

        let mut request = self.client.request(method.clone(), url);
        if let Value::Object(map) = &params {
            let query: Vec<(String, String)> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_string(v)))
                .collect();
            request = request.query(&query);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ExecFailure::failed(format!("HTTP isteği başarısız: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(ExecFailure::failed(format!("HTTP {status}")));
        }
        response
            .json::<Value>()
            .await
            .or(Ok(json!({})))
    }

    async fn run_sql(&self, def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
        // Faz 2: config.connection verilmişse o bağlantıya bağlan
        if let Some(conn_id) = def.config.get("connection").and_then(Value::as_str) {
            if !conn_id.is_empty() {
                return self.run_sql_on_connection(conn_id, def, env).await;
            }
        }

        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| ExecFailure::failed("sql autoexec için veritabanı havuzu yok"))?;
        let query = def
            .config
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ExecFailure::failed("sql config'te query yok"))?;
        let params = def
            .config
            .get("params")
            .map(|p| resolve_config_value(p, env))
            .unwrap_or(json!({}));

        // :name parametrelerini sıralı $n bind'lerine çevir
        let mut bound: Vec<Value> = Vec::new();
        let mut sql = query.to_string();
        if let Value::Object(map) = &params {
            for (name, value) in map {
                let placeholder = format!(":{name}");
                if sql.contains(&placeholder) {
                    bound.push(value.clone());
                    sql = sql.replace(&placeholder, &format!("${}", bound.len()));
                }
            }
        }

        let mut q = sqlx::query(&sql);
        for value in &bound {
            q = match value {
                Value::Number(n) if n.is_i64() => q.bind(n.as_i64()),
                Value::Number(n) => q.bind(n.as_f64()),
                Value::Bool(b) => q.bind(*b),
                Value::String(s) => q.bind(s.clone()),
                other => q.bind(other.clone()),
            };
        }

        let rows = q
            .fetch_all(pool)
            .await
            .map_err(|e| ExecFailure::failed(format!("sql hatası: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in &rows {
            let mut obj = Map::new();
            for column in row.columns() {
                let value: Value = row
                    .try_get::<Value, _>(column.name())
                    .or_else(|_| row.try_get::<String, _>(column.name()).map(Value::from))
                    .or_else(|_| row.try_get::<i64, _>(column.name()).map(Value::from))
                    .or_else(|_| row.try_get::<f64, _>(column.name()).map(Value::from))
                    .or_else(|_| row.try_get::<bool, _>(column.name()).map(Value::from))
                    .unwrap_or(Value::Null);
                obj.insert(column.name().to_string(), value);
            }
            out.push(Value::Object(obj));
        }

        // Tek satır → obje ($exec.result.field erişimi için), çoklu → {rows: [...]}
        Ok(match out.len() {
            1 => out.into_iter().next().unwrap(),
            _ => json!({ "rows": out }),
        })
    }

    async fn run_sql_on_connection(
        &self,
        conn_id: &str,
        def: &AutoexecDef,
        env: &ExecEnv,
    ) -> Result<Value, ExecFailure> {
        let id = Uuid::parse_str(conn_id)
            .map_err(|_| ExecFailure::failed(format!("geçersiz connection id: {conn_id}")))?;
        let meta_pool = self
            .pool
            .as_ref()
            .ok_or_else(|| ExecFailure::failed("connection çözümü için meta havuzu yok"))?;

        // Bağlantı satırını oku (updated_at ile önbellek anahtarı)
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
                chrono::DateTime<chrono::Utc>,
            ),
        >(
            "SELECT driver, mode, host, port, database, username, options, secret_enc, updated_at \
             FROM wf.db_connection WHERE id = $1 AND is_active = true",
        )
        .bind(id)
        .fetch_optional(meta_pool)
        .await
        .map_err(|e| ExecFailure::failed(format!("connection okunamadı: {e}")))?
        .ok_or_else(|| ExecFailure::failed(format!("connection bulunamadı: {conn_id}")))?;

        let updated = row.8.timestamp();
        let handle = {
            let mut reg = self.registry.lock().await;
            match reg.get(&id) {
                Some((ts, h)) if *ts == updated => h.clone(),
                _ => {
                    let driver = DbDriver::parse(&row.0)
                        .ok_or_else(|| ExecFailure::failed("geçersiz driver"))?;
                    let secret = match &row.7 {
                        Some(b) => Some(
                            db::crypto::decrypt(b)
                                .map_err(|e| ExecFailure::failed(format!("secret çözülemedi: {e}")))?,
                        ),
                        None => None,
                    };
                    let cfg = DbConfig {
                        driver,
                        mode: row.1.clone(),
                        host: row.2.clone(),
                        port: row.3,
                        database: row.4.clone(),
                        username: row.5.clone(),
                        secret,
                        options: row.6.clone(),
                    };
                    let h = Arc::new(
                        db::run::connect(&cfg)
                            .await
                            .map_err(|e| ExecFailure::failed(format!("bağlanılamadı: {e}")))?,
                    );
                    reg.insert(id, (updated, h.clone()));
                    h
                }
            }
        };

        let query = def
            .config
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| ExecFailure::failed("sql config'te query yok"))?;
        let params = def
            .config
            .get("params")
            .map(|p| resolve_config_value(p, env))
            .unwrap_or(json!({}));
        let empty = Map::new();
        let params_map = params.as_object().unwrap_or(&empty);
        db::run::run_query(&handle, query, params_map)
            .await
            .map_err(|e| ExecFailure::failed(format!("sql hatası: {e}")))
    }
}

fn run_calc(def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
    let expressions = def
        .config
        .get("expressions")
        .and_then(Value::as_object)
        .ok_or_else(|| ExecFailure::failed("calc config'te expressions yok"))?;

    let eval_env = EvalEnv::new(&env.ctx)
        .with_node(env.node.as_deref())
        .with_actor(&env.actor)
        .with_wfe_id(env.wfe_id);

    let mut result = Map::new();
    for (name, expr) in expressions {
        let expr = expr
            .as_str()
            .ok_or_else(|| ExecFailure::failed(format!("calc ifadesi string olmalı: {name}")))?;
        let value = evaluate_value(expr, &eval_env)
            .map_err(|e| ExecFailure::failed(format!("calc '{name}': {e}")))?;
        result.insert(name.clone(), value);
    }
    Ok(Value::Object(result))
}

/// Config değerlerindeki $-string'leri çözer ($ctx.*, $wfe_id, $actor, $node, $timestamp).
fn resolve_config_value(raw: &Value, env: &ExecEnv) -> Value {
    match raw {
        Value::String(s) => resolve_config_string(s, env),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), resolve_config_value(v, env)))
                .collect(),
        ),
        Value::Array(arr) => {
            Value::Array(arr.iter().map(|v| resolve_config_value(v, env)).collect())
        }
        other => other.clone(),
    }
}

fn resolve_config_string(s: &str, env: &ExecEnv) -> Value {
    match s {
        "$wfe_id" => Value::from(env.wfe_id.to_string()),
        "$actor" => serde_json::to_value(&env.actor).unwrap_or(Value::Null),
        "$node" => env.node.as_deref().map(Value::from).unwrap_or(Value::Null),
        "$timestamp" => Value::from(chrono::Utc::now().to_rfc3339()),
        _ => {
            if let Some(path) = s.strip_prefix("$ctx.") {
                let mut current = &env.ctx;
                for part in path.split('.') {
                    match current.get(part) {
                        Some(v) => current = v,
                        None => return Value::Null,
                    }
                }
                return current.clone();
            }
            Value::from(s)
        }
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use wfe_core::types::actor::Actor;

    fn env(ctx: Value) -> ExecEnv {
        ExecEnv {
            wfe_id: Uuid::nil(),
            ctx,
            node: Some("self__creditAnalyst".into()),
            actor: Actor {
                orgu_id: Uuid::nil(),
                user_id: Uuid::nil(),
                role: "system".into(),
            },
        }
    }

    #[tokio::test]
    async fn calc_evaluates_fixture_expression() {
        let runner = LiveAutoexecRunner::new(None);
        let def: AutoexecDef = serde_json::from_value(json!({
            "type": "calc",
            "config": {
                "expressions": {
                    "within_limit": "$ctx.credit_score >= 700 and $ctx.credit_info.amount_requested <= 50000"
                }
            }
        }))
        .unwrap();
        let e = env(json!({"credit_score": 750, "credit_info": {"amount_requested": 30000}}));
        let result = runner.run(&def, &e).await.unwrap();
        assert_eq!(result, json!({"within_limit": true}));
    }

    #[tokio::test]
    async fn unsupported_type_fails_cleanly() {
        let runner = LiveAutoexecRunner::new(None);
        let def: AutoexecDef = serde_json::from_value(json!({
            "type": "python",
            "config": {}
        }))
        .unwrap();
        let err = runner.run(&def, &env(json!({}))).await.unwrap_err();
        assert_eq!(err.error, "WFD.AutoexecFailed");
    }

    #[test]
    fn config_dollar_refs_resolve() {
        let e = env(json!({"applicant": {"tckid": "12345678901"}}));
        let resolved = resolve_config_value(
            &json!({"tckid": "$ctx.applicant.tckid", "wfe": "$wfe_id", "plain": "x"}),
            &e,
        );
        assert_eq!(resolved["tckid"], json!("12345678901"));
        assert_eq!(resolved["wfe"], json!(Uuid::nil().to_string()));
        assert_eq!(resolved["plain"], json!("x"));
    }
}
