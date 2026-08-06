//! v2.2 AutoexecRunner — REST / SQL / CALC (WOR-41).
//! Timeout PIPELINE tarafından uygulanır (M5); burada yalnızca ham çalıştırma var.
//! Config içindeki $-string parametreleri ExecEnv ctx'i ile çözülür.

use async_trait::async_trait;
use reqwest::{Client, Method};
use serde_json::{json, Map, Value};
use sqlx::PgPool;
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
    // connection_id → (updated_at_epoch, çözülmüş config parmak izi, handle).
    // Parmak izi ORTAMI kapsar: `$env` ile şablonlanmış bir bağlantı test ve prod'da farklı
    // bir hedefe çözülür, tek anahtarla önbelleklenirse prod sorgusu test DB'sine giderdi.
    registry: Mutex<HashMap<Uuid, (i64, u64, Arc<RunHandle>)>>,
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
        let method = def
            .config
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("GET");
        let method = Method::from_str(method)
            .map_err(|_| ExecFailure::failed(format!("geçersiz HTTP metodu: {method}")))?;
        let url = def
            .config
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| ExecFailure::failed("rest config'te url yok"))?;
        // URL'nin kendisi de `$env` taşır — asıl kullanım vakası bu.
        let url = match resolve_config_string(url, env)? {
            Value::String(s) => s,
            other => value_to_string(&other),
        };

        let params = def
            .config
            .get("params")
            .map(|p| resolve_config_value(p, env))
            .transpose()?
            .unwrap_or(Value::Null);
        let body = def
            .config
            .get("body")
            .map(|b| resolve_config_value(b, env))
            .transpose()?;
        let form = def
            .config
            .get("form")
            .map(|f| resolve_config_value(f, env))
            .transpose()?;
        let headers = def
            .config
            .get("headers")
            .map(|h| resolve_config_value(h, env))
            .transpose()?
            .unwrap_or(Value::Null);

        let mut request = self.client.request(method.clone(), &url);
        if let Value::Object(map) = &params {
            let query: Vec<(String, String)> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_string(v)))
                .collect();
            request = request.query(&query);
        }
        if let Value::Object(map) = &headers {
            for (k, v) in map {
                request = request.header(k, value_to_string(v));
            }
        }
        request = apply_auth(request, &def.config, env)?;
        if let Some(Value::Object(map)) = &form {
            let pairs: Vec<(String, String)> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_string(v)))
                .collect();
            request = request.form(&pairs);
        } else if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request
            .send()
            .await
            .map_err(|e| ExecFailure::failed(format!("HTTP isteği başarısız: {e}")))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            // Yanıt gövdesi gönderdiğimiz secret'ı geri yankılayabilir (ör. "invalid token:
            // sk-live-…"). Hata metni WFAH'a ve portala gider — maskelenmeden geçemez.
            let snippet: String = text.chars().take(500).collect();
            let snippet = mask_in_str(&snippet, &env.env.full().secret_strings());
            return Err(ExecFailure::failed(if snippet.is_empty() {
                format!("HTTP {status}")
            } else {
                format!("HTTP {status}: {snippet}")
            }));
        }
        // JSON yanıt → doğrudan result; değilse ham gövde $exec.result.body altında
        Ok(serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
            if text.is_empty() {
                json!({})
            } else {
                json!({ "body": text })
            }
        }))
    }

    async fn run_sql(&self, def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
        // Faz 2: config.connection verilmişse o bağlantıya bağlan
        if let Some(conn_id) = def.config.get("connection").and_then(Value::as_str) {
            if !conn_id.is_empty() {
                return self.run_sql_on_connection(conn_id, def, env).await;
            }
        }

        // Varsayılan: engine'in kendi Postgres havuzu (db::run ile aynı yol)
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
            .transpose()?
            .unwrap_or(json!({}));
        let empty = Map::new();
        let params_map = params.as_object().unwrap_or(&empty);
        db::run::run_query(&RunHandle::Pg(pool.clone()), query, params_map)
            .await
            .map_err(|e| ExecFailure::failed(format!("sql hatası: {e}")))
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
                // port artık `text`: `$env.PG_PORT` gibi bir şablon tutabilir, çözümden
                // sonra parse edilir (migration 20260804000002).
                Option<String>,
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
        let driver =
            DbDriver::parse(&row.0).ok_or_else(|| ExecFailure::failed("geçersiz driver"))?;
        let secret = match &row.7 {
            Some(b) => Some(
                db::crypto::decrypt(b)
                    .map_err(|e| ExecFailure::failed(format!("secret çözülemedi: {e}")))?,
            ),
            None => None,
        };

        // Bağlantı alanları `$env` ile şablonlanabilir: TEK bir db_connection satırı tüm
        // ortamlara hizmet eder (host='$env.MONGO_HOST', secret='$env.MONGO_PW'). Bu yol
        // autoexec config yoludur, dolayısıyla secret'lar DAHİL çözülür.
        let cfg = DbConfig {
            driver,
            mode: row.1.clone(),
            host: resolve_conn_field(row.2.as_deref(), env)?,
            port: resolve_conn_port(row.3.as_deref(), env)?,
            database: resolve_conn_field(row.4.as_deref(), env)?,
            username: resolve_conn_field(row.5.as_deref(), env)?,
            secret: resolve_conn_field(secret.as_deref(), env)?,
            options: resolve_config_value(&row.6, env)?,
        };

        // Önbellek anahtarı ortamı da kapsamalı: aynı bağlantı satırı test ve prod'da FARKLI
        // bir hedefe çözülür. Ortam kimliği `ExecEnv`'de taşınmadığı için çözülmüş config'in
        // parmak izi kullanılır — secret'ı anahtar olarak bellekte düz tutmamak için hash.
        let fingerprint = config_fingerprint(&cfg);
        let handle = {
            let mut reg = self.registry.lock().await;
            match reg.get(&id) {
                Some((ts, fp, h)) if *ts == updated && *fp == fingerprint => h.clone(),
                _ => {
                    let h = Arc::new(
                        db::run::connect(&cfg)
                            .await
                            .map_err(|e| ExecFailure::failed(format!("bağlanılamadı: {e}")))?,
                    );
                    reg.insert(id, (updated, fingerprint, h.clone()));
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
            .transpose()?
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

    // WOR-84: `$wfah`/`$prev`/`$first` ve `$action.input.*` BAĞLANIR. Öncesinde yalnız
    // ctx/node/actor/wfe_id bağlıydı; `$wfah` kullanan calc ifadesi sessizce null okuyup
    // yanlış hesaplıyordu (`len($wfah)` ise patlıyordu).
    let mut eval_env = EvalEnv::new(&env.ctx)
        .with_wfah(&env.wfah)
        .with_node(env.node.as_deref())
        .with_actor(&env.actor)
        .with_wfe_id(env.wfe_id)
        // SECRET'SIZ görünüm: calc sonucu `wfes_effects` ile ctx'e yazılır, oradan portala
        // görünür. Secret bir değerin bu yoldan geçmesi maskelemenin tamamını boşa çıkarırdı.
        .with_env(env.env.public());
    if let Some(input) = &env.action_input {
        eval_env = eval_env.with_action_input(input);
    }

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

/// REST auth kısayolu: config.auth {"type": "bearer"|"basic"|"api_key", ...}.
/// headers ile birlikte kullanılabilir; auth en son uygulanır (aynı header'ı ezer).
fn apply_auth(
    request: reqwest::RequestBuilder,
    config: &Value,
    env: &ExecEnv,
) -> Result<reqwest::RequestBuilder, ExecFailure> {
    let Some(auth) = config.get("auth") else {
        return Ok(request);
    };
    let auth = resolve_config_value(auth, env)?;
    let kind = auth.get("type").and_then(Value::as_str).unwrap_or("");
    let get = |k: &str| {
        auth.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match kind {
        "" | "none" => Ok(request),
        "bearer" => Ok(request.bearer_auth(get("token"))),
        "basic" => {
            let password = auth.get("password").and_then(Value::as_str);
            Ok(request.basic_auth(get("username"), password))
        }
        "api_key" => {
            let header = auth
                .get("header")
                .and_then(Value::as_str)
                .unwrap_or("X-API-Key");
            Ok(request.header(header, get("value")))
        }
        other => Err(ExecFailure::failed(format!("geçersiz auth tipi: {other}"))),
    }
}

/// Test endpoint'i için: config'in $-string'leri çözülmüş hâli (request_info).
///
/// Secret `$env` değerleri `[MASKED]` ile değiştirilir. Bu, secret sınırının ikinci
/// yarısıdır: birinci yarı (secret'ın ctx'e yazılamaması) tip düzeyinde sağlanır, ama
/// autoexec config'i secret'ı MEŞRU olarak çözer (Authorization header'ı) — o çözülmüş
/// hâlin tasarımcının ekranına dönmesi sızıntı olurdu.
pub fn resolved_config(def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure> {
    let resolved = resolve_config_value(&def.config, env)?;
    Ok(mask_secrets(resolved, env))
}

/// Çözülmüş secret değerleri metin içinde `[MASKED]` ile değiştirir.
/// GitLab'ın "masked variable" kuralının karşılığı; kısa/boşluklu değerler API katmanında
/// zaten reddedilir (aksi hâlde maskeleme log'u kullanılamaz hâle getirirdi).
pub fn mask_secrets(v: Value, env: &ExecEnv) -> Value {
    let secrets = env.env.full().secret_strings();
    if secrets.is_empty() {
        return v;
    }
    mask_in_value(v, &secrets)
}

fn mask_in_value(v: Value, secrets: &[String]) -> Value {
    match v {
        Value::String(s) => Value::String(mask_in_str(&s, secrets)),
        Value::Object(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| (k, mask_in_value(v, secrets)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| mask_in_value(v, secrets)).collect()),
        other => other,
    }
}

pub(crate) fn mask_in_str(s: &str, secrets: &[String]) -> String {
    let mut out = s.to_string();
    for sec in secrets {
        if out.contains(sec.as_str()) {
            out = out.replace(sec.as_str(), "[MASKED]");
        }
    }
    out
}

/// Config değerlerindeki $-string'leri çözer
/// ($env.*, $ctx.*, $wfe_id, $actor, $node, $timestamp).
///
/// `Result` döner çünkü `$env` tanımsız anahtarda HATA verir (bkz. `wfe_core::v22::env`):
/// sessizce `null` dönmek `https://null/v1/users` gibi bir istek üretirdi.
fn resolve_config_value(raw: &Value, env: &ExecEnv) -> Result<Value, ExecFailure> {
    match raw {
        Value::String(s) => resolve_config_string(s, env),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_config_value(v, env)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(arr) => Ok(Value::Array(
            arr.iter()
                .map(|v| resolve_config_value(v, env))
                .collect::<Result<_, _>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_config_string(s: &str, env: &ExecEnv) -> Result<Value, ExecFailure> {
    // `$env` EN BAŞTA ve secret'lar DAHİL: autoexec config, secret bir değerin çözülebildiği
    // TEK yoldur (ZEN ve effects yalnız `PublicEnv` görür). Diğer $-formlarının aksine
    // ara-değer de çözülür — "$env.AUTH_API/v1/users".
    //
    // Tanımsız anahtar HATADIR, `null` değil: null bir domain `https://null/v1/users`
    // üretir ya da daha kötüsü yanlış bir hosta gider. Publish-time validasyonu
    // (`validator::env_references`) bunu zaten yakalar; burası son savunma hattı.
    match wfe_core::v22::env::resolve_string(s, env.env.full()) {
        Ok(Some(v)) => return Ok(v),
        Ok(None) => {}
        Err(e) => return Err(ExecFailure::failed(e.to_string())),
    }
    Ok(match s {
        "$wfe_id" => Value::from(env.wfe_id.to_string()),
        "$actor" => serde_json::to_value(&env.actor).unwrap_or(Value::Null),
        "$node" => env.node.as_deref().map(Value::from).unwrap_or(Value::Null),
        "$timestamp" => Value::from(wfe_core::timestamp::now_timestamp()),
        _ => {
            if let Some(path) = s.strip_prefix("$ctx.") {
                let mut current = &env.ctx;
                for part in path.split('.') {
                    match current.get(part) {
                        Some(v) => current = v,
                        None => return Ok(Value::Null),
                    }
                }
                return Ok(current.clone());
            }
            Value::from(s)
        }
    })
}

/// Bir `db_connection` metin alanını `$env` ile çözer. `None` alan `None` kalır.
fn resolve_conn_field(raw: Option<&str>, env: &ExecEnv) -> Result<Option<String>, ExecFailure> {
    let Some(s) = raw else { return Ok(None) };
    Ok(Some(match resolve_config_string(s, env)? {
        Value::String(v) => v,
        other => value_to_string(&other),
    }))
}

/// Port: çözümden SONRA parse edilir. Kolon `text` çünkü şablon tutabilir, ama bağlantı
/// kurulurken sayı olmak zorunda.
fn resolve_conn_port(raw: Option<&str>, env: &ExecEnv) -> Result<Option<i32>, ExecFailure> {
    let Some(s) = resolve_conn_field(raw, env)? else {
        return Ok(None);
    };
    if s.is_empty() {
        return Ok(None);
    }
    s.parse::<i32>()
        .map(Some)
        .map_err(|_| ExecFailure::failed(format!("bağlantı portu sayı değil: '{s}'")))
}

/// Çözülmüş bağlantı config'inin parmak izi — önbellek anahtarının ortam duyarlı parçası.
/// Secret düz metin olarak bellekte anahtar tutulmasın diye hash'lenir.
fn config_fingerprint(cfg: &DbConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cfg.mode.hash(&mut h);
    cfg.host.hash(&mut h);
    cfg.port.hash(&mut h);
    cfg.database.hash(&mut h);
    cfg.username.hash(&mut h);
    cfg.secret.hash(&mut h);
    cfg.options.to_string().hash(&mut h);
    h.finish()
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
    use wfe_core::types::wfah::Wfah;

    fn env(ctx: Value) -> ExecEnv {
        ExecEnv {
            env: Default::default(),
            wfe_id: Uuid::nil(),
            ctx,
            node: Some("self__creditAnalyst".into()),
            actor: Actor {
                orgu_id: Uuid::nil(),
                user_id: Uuid::nil(),
                role: "system".into(),
            },
            wfah: Wfah::empty(),
            action_input: None,
        }
    }

    fn actor_of(role: &str) -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: role.into(),
        }
    }

    /// WOR-84: calc ifadeleri geçmişi görür. Öncesinde `$wfah` bağlı değildi —
    /// `len($wfah)` patlıyor, `$prev.action` null okuyordu.
    #[tokio::test]
    async fn calc_sees_wfah_prev_and_action_input() {
        let mut e = env(serde_json::json!({"limit": 1000}));
        e.wfah = Wfah::empty()
            .push("basvuru".into(), actor_of("clerk"), None)
            .push(
                "skor_gir".into(),
                actor_of("analyst"),
                Some(serde_json::json!({"skor": 720})),
            );
        e.action_input = Some(serde_json::json!({"tutar": 400}));

        let def: AutoexecDef = serde_json::from_value(serde_json::json!({
            "type": "calc",
            "config": { "expressions": {
                "adim_sayisi": "len($wfah)",
                "onceki_aksiyon": "$prev.action",
                "onceki_skor": "$prev.input.skor",
                "ilk_aksiyon": "$first.action",
                "onaylandi": "some($wfah, #.actor.role == 'analyst')",
                "limit_ici": "$action.input.tutar <= $ctx.limit",
            }}
        }))
        .unwrap();

        let out = LiveAutoexecRunner::new(None).run(&def, &e).await.unwrap();
        assert_eq!(out["adim_sayisi"], 2);
        assert_eq!(out["onceki_aksiyon"], "skor_gir");
        assert_eq!(out["onceki_skor"], 720);
        assert_eq!(out["ilk_aksiyon"], "basvuru");
        assert_eq!(out["onaylandi"], true);
        assert_eq!(out["limit_ici"], true);
    }

    /// Boş geçmişte calc PATLAMAZ — `$prev.*` null kabuğu okur.
    #[tokio::test]
    async fn calc_prev_is_null_on_empty_history() {
        let e = env(serde_json::json!({}));
        let def: AutoexecDef = serde_json::from_value(serde_json::json!({
            "type": "calc",
            "config": { "expressions": {
                "adim_sayisi": "len($wfah)",
                "ilk_mi": "$prev.action == null",
            }}
        }))
        .unwrap();
        let out = LiveAutoexecRunner::new(None).run(&def, &e).await.unwrap();
        assert_eq!(out["adim_sayisi"], 0);
        assert_eq!(out["ilk_mi"], true);
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
    fn auth_bearer_and_api_key_set_headers() {
        let client = Client::new();
        let e = env(json!({"tok": "abc"}));

        let req = apply_auth(
            client.get("http://x/"),
            &json!({"auth": {"type": "bearer", "token": "$ctx.tok"}}),
            &e,
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(req.headers()["authorization"], "Bearer abc");

        let req = apply_auth(
            client.get("http://x/"),
            &json!({"auth": {"type": "api_key", "value": "k1"}}),
            &e,
        )
        .unwrap()
        .build()
        .unwrap();
        assert_eq!(req.headers()["x-api-key"], "k1");

        let req = apply_auth(
            client.get("http://x/"),
            &json!({"auth": {"type": "basic", "username": "u", "password": "p"}}),
            &e,
        )
        .unwrap()
        .build()
        .unwrap();
        assert!(req.headers()["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Basic "));

        let err = apply_auth(
            client.get("http://x/"),
            &json!({"auth": {"type": "oauth9"}}),
            &e,
        )
        .unwrap_err();
        assert!(err.message.contains("geçersiz auth tipi"));

        // auth yoksa dokunulmaz
        let req = apply_auth(client.get("http://x/"), &json!({}), &e)
            .unwrap()
            .build()
            .unwrap();
        assert!(req.headers().get("authorization").is_none());
    }

    #[test]
    fn config_dollar_refs_resolve() {
        let e = env(json!({"applicant": {"tckid": "12345678901"}}));
        let resolved = resolve_config_value(
            &json!({"tckid": "$ctx.applicant.tckid", "wfe": "$wfe_id", "plain": "x"}),
            &e,
        )
        .unwrap();
        assert_eq!(resolved["tckid"], json!("12345678901"));
        assert_eq!(resolved["wfe"], json!(Uuid::nil().to_string()));
        assert_eq!(resolved["plain"], json!("x"));
    }

    fn env_with_vars(ctx: Value) -> ExecEnv {
        use wfe_core::v22::env::{EnvSet, EnvValue, RunEnv};
        let mut e = env(ctx);
        e.env = RunEnv::new(EnvSet::new(std::collections::BTreeMap::from([
            (
                "SCORE_API".to_string(),
                EnvValue::public(json!("https://skor.test.cs.com.tr")),
            ),
            ("RETRIES".to_string(), EnvValue::public(json!(3))),
            (
                "API_KEY".to_string(),
                EnvValue::secret(json!("sk-live-abc12345")),
            ),
        ])));
        e
    }

    /// Autoexec config `$env`'i secret'lar DAHİL çözer — secret'ın çözülebildiği TEK yol.
    #[test]
    fn env_resolves_in_config_including_secrets() {
        let e = env_with_vars(json!({}));
        let resolved = resolve_config_value(
            &json!({
                "url": "$env.SCORE_API/v1/score",
                "retries": "$env.RETRIES",
                "headers": { "Authorization": "Bearer $env.API_KEY" }
            }),
            &e,
        )
        .unwrap();
        assert_eq!(resolved["url"], json!("https://skor.test.cs.com.tr/v1/score"));
        assert_eq!(resolved["retries"], json!(3), "tam eşleşme tipi korur");
        assert_eq!(
            resolved["headers"]["Authorization"],
            json!("Bearer sk-live-abc12345")
        );
    }

    /// KRİTİK: aynı config test endpoint'ine dönerken secret MASKELENİR. Çözülmüş hâlin
    /// tasarımcının ekranına dönmesi sızıntı olurdu.
    #[test]
    fn resolved_config_masks_secrets() {
        let e = env_with_vars(json!({}));
        let def: AutoexecDef = serde_json::from_value(json!({
            "type": "rest",
            "config": {
                "url": "$env.SCORE_API/v1",
                "headers": { "Authorization": "Bearer $env.API_KEY" }
            }
        }))
        .unwrap();
        let shown = resolved_config(&def, &e).unwrap();
        assert_eq!(shown["url"], json!("https://skor.test.cs.com.tr/v1"));
        assert_eq!(
            shown["headers"]["Authorization"],
            json!("Bearer [MASKED]"),
            "secret ekrana dönmemeli"
        );
    }

    /// Tanımsız anahtar sessizce `null` OLMAZ — `https://null/v1` isteği atılmasın.
    #[test]
    fn undefined_env_key_fails_config_resolution() {
        let e = env_with_vars(json!({}));
        let err = resolve_config_value(&json!({"url": "$env.YOK/v1"}), &e).unwrap_err();
        assert!(format!("{err:?}").contains("YOK"), "{err:?}");
    }

    /// `calc` ZEN üzerinden koşar: secret'ı GÖREMEZ (ctx'e yazılabilirdi).
    #[tokio::test]
    async fn calc_cannot_read_secret_env() {
        let e = env_with_vars(json!({}));
        let def: AutoexecDef = serde_json::from_value(json!({
            "type": "calc",
            "config": { "expressions": { "sizan": "$env.API_KEY" } }
        }))
        .unwrap();
        assert!(LiveAutoexecRunner::new(None).run(&def, &e).await.is_err());

        // Secret olmayan anahtar okunur.
        let def: AutoexecDef = serde_json::from_value(json!({
            "type": "calc",
            "config": { "expressions": { "deneme": "$env.RETRIES + 1" } }
        }))
        .unwrap();
        let out = LiveAutoexecRunner::new(None).run(&def, &e).await.unwrap();
        assert_eq!(out["deneme"], json!(4));
    }
}
