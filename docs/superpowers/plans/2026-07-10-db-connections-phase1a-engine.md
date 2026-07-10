# DB Bağlantı Entegrasyonu — Faz 1a (Engine) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Engine'de çok-sürücülü (Postgres/MySQL/MSSQL) DB bağlantı deposu (AES-256-GCM şifreli), CRUD + test API'si ve sürücü soyutlaması.

**Architecture:** Yeni `crates/wfe/src/db/` modülü: saf `crypto` (şifreleme), `DbDriver` enum + `DbConn` trait + üç sürücü impl (Postgres/MySQL `sqlx`, MSSQL `tiberius`), ve `registry`. `wf.db_connection` tablosu tenant-scoped; secret şifreli, API'de asla dönmez. `/db/connections` router server crate'inde, `/org` gibi X-Admin-Key korumalı.

**Tech Stack:** Rust, Axum, sqlx (postgres+mysql), tiberius (mssql), aes-gcm, tokio. Migration psql (docker: `docker exec -i wf-engine-postgres psql -U apex -d wf_engine`), `psql` PATH'te yok.

**Kapsam notu:** Bu plan Faz 1a (engine backend). Faz 1b (editör UI) ve Faz 2 (SQL node runtime bağlama) ayrı planlar. Spec: `docs/superpowers/specs/2026-07-10-wfd-db-connections-design.md`.

---

## Dosya Haritası

- **Create:** `migrations/wf/20260710000002_db_connection.sql` — tablo.
- **Modify:** `Cargo.toml` (workspace) — sqlx `mysql` feature; yeni workspace deps: `tiberius`, `tokio-util`, `aes-gcm`, `rand`, `base64`.
- **Modify:** `crates/wfe/Cargo.toml` — yeni deps.
- **Create:** `crates/wfe/src/db/mod.rs` — `DbDriver`, `DbConfig`, `DbConn` trait, `test_connection()` dispatcher, hata tipi.
- **Create:** `crates/wfe/src/db/crypto.rs` — `encrypt`/`decrypt` (AES-256-GCM), saf birim testli.
- **Create:** `crates/wfe/src/db/drivers.rs` — Postgres/MySQL (sqlx) + MSSQL (tiberius) `test()` implementasyonları.
- **Modify:** `crates/wfe/src/lib.rs` — `pub mod db;`.
- **Create:** `crates/server/src/routes/db.rs` — `/db/connections` CRUD + test handler'ları + repo sorguları.
- **Modify:** `crates/server/src/main.rs` — `/db` nest (X-Admin-Key korumalı).
- **Modify:** `crates/server/src/config.rs` — `db_conn_secret` env okuma (gerekirse).

---

## Task 1: Migration — `wf.db_connection`

**Files:** Create `migrations/wf/20260710000002_db_connection.sql`

- [ ] **Step 1: Migration dosyasını yaz**

```sql
-- DB bağlantı deposu (2026-07-10 tasarımı). secret_enc: AES-256-GCM (nonce||ciphertext).
CREATE TABLE wf.db_connection (
    id           uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    name         text        NOT NULL,
    driver       text        NOT NULL CHECK (driver IN ('postgres','mysql','mssql')),
    mode         text        NOT NULL DEFAULT 'fields' CHECK (mode IN ('fields','uri')),
    host         text,
    port         integer,
    database     text,
    username     text,
    options      jsonb       NOT NULL DEFAULT '{}',
    secret_enc   bytea,
    is_active    boolean     NOT NULL DEFAULT true,
    last_test_at timestamptz,
    last_test_ok boolean,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX db_connection_orgtnt_idx ON wf.db_connection(orgtnt_id);
```

- [ ] **Step 2: Uygula**

Run: `docker exec -i wf-engine-postgres psql -U apex -d wf_engine -f - < migrations/wf/20260710000002_db_connection.sql`
Expected: `CREATE TABLE` / `CREATE INDEX`, hata yok.

- [ ] **Step 3: Doğrula**

Run: `docker exec -i wf-engine-postgres psql -U apex -d wf_engine -c "\d wf.db_connection"`
Expected: tüm kolonlar + unique (orgtnt_id,name) + index.

- [ ] **Step 4: Commit**

```bash
git add migrations/wf/20260710000002_db_connection.sql
git commit -m "feat(wf): db_connection tablosu (şifreli çok-sürücü bağlantı deposu)"
```

---

## Task 2: Bağımlılıklar

**Files:** Modify `Cargo.toml`, `crates/wfe/Cargo.toml`

- [ ] **Step 1: Workspace deps**

`Cargo.toml` `[workspace.dependencies]` içinde `sqlx` satırının features'ına `"mysql"` ekle ve şunları ekle:

```toml
sqlx      = { version = "0.7", features = ["postgres", "mysql", "runtime-tokio-rustls", "uuid", "chrono", "json"] }
tiberius  = { version = "0.12", default-features = false, features = ["rustls", "chrono"] }
tokio-util = { version = "0.7", features = ["compat"] }
aes-gcm   = "0.10"
rand      = "0.8"
base64    = "0.22"
```

- [ ] **Step 2: wfe crate deps**

`crates/wfe/Cargo.toml` `[dependencies]`'e ekle:

```toml
tiberius   = { workspace = true }
tokio-util = { workspace = true }
aes-gcm    = { workspace = true }
rand       = { workspace = true }
base64     = { workspace = true }
```

- [ ] **Step 3: Derleme (deps çözülür)**

Run: `cargo build -p wf-wfe 2>&1 | tail -5`
Expected: yeni crate'ler indirilir, derleme geçer (henüz kullanılmıyor — uyarı olmaz çünkü Cargo dep'i kullanmadan uyarmaz).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml crates/wfe/Cargo.toml Cargo.lock
git commit -m "chore(wfe): sqlx mysql + tiberius/aes-gcm bağımlılıkları"
```

---

## Task 3: Şifreleme — `crypto.rs` (saf, TDD)

**Files:** Create `crates/wfe/src/db/crypto.rs`, Modify `crates/wfe/src/lib.rs`

- [ ] **Step 1: `lib.rs`'e modül ekle**

`crates/wfe/src/lib.rs`'e ekle: `pub mod db;`

Ve `crates/wfe/src/db/mod.rs` oluştur (şimdilik sadece): `pub mod crypto;`

- [ ] **Step 2: Başarısız testi yaz**

`crates/wfe/src/db/crypto.rs`:

```rust
//! Bağlantı secret'ları için AES-256-GCM şifreleme.
//! Anahtar env `DB_CONN_SECRET` (base64, 32 byte). Format: nonce(12) || ciphertext.
use aes_gcm::{aead::{Aead, KeyInit, OsRng, rand_core::RngCore}, Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};

#[derive(Debug)]
pub enum CryptoError { NoKey, BadKey, Encrypt, Decrypt }

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CryptoError::NoKey => "DB_CONN_SECRET tanımlı değil",
            CryptoError::BadKey => "DB_CONN_SECRET geçersiz (base64 32 byte olmalı)",
            CryptoError::Encrypt => "şifreleme hatası",
            CryptoError::Decrypt => "çözme hatası (anahtar/veri uyumsuz)",
        };
        write!(f, "{s}")
    }
}
impl std::error::Error for CryptoError {}

fn key_from(b64: &str) -> Result<Key<Aes256Gcm>, CryptoError> {
    let raw = STANDARD.decode(b64.trim()).map_err(|_| CryptoError::BadKey)?;
    if raw.len() != 32 { return Err(CryptoError::BadKey); }
    Ok(*Key::<Aes256Gcm>::from_slice(&raw))
}

/// Verilen anahtarla şifreler → nonce||ciphertext byte'ları.
pub fn encrypt_with(key_b64: &str, plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ct = cipher.encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
        .map_err(|_| CryptoError::Encrypt)?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(out)
}

/// nonce||ciphertext → düz metin.
pub fn decrypt_with(key_b64: &str, data: &[u8]) -> Result<String, CryptoError> {
    if data.len() < 13 { return Err(CryptoError::Decrypt); }
    let cipher = Aes256Gcm::new(&key_from(key_b64)?);
    let (nonce, ct) = data.split_at(12);
    let pt = cipher.decrypt(Nonce::from_slice(nonce), ct).map_err(|_| CryptoError::Decrypt)?;
    String::from_utf8(pt).map_err(|_| CryptoError::Decrypt)
}

/// Env'den anahtar okuyan sarmalayıcılar.
pub fn encrypt(plaintext: &str) -> Result<Vec<u8>, CryptoError> {
    let k = std::env::var("DB_CONN_SECRET").map_err(|_| CryptoError::NoKey)?;
    encrypt_with(&k, plaintext)
}
pub fn decrypt(data: &[u8]) -> Result<String, CryptoError> {
    let k = std::env::var("DB_CONN_SECRET").map_err(|_| CryptoError::NoKey)?;
    decrypt_with(&k, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    // 32 byte base64 (deterministik test anahtarı)
    const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

    #[test]
    fn round_trip() {
        let ct = encrypt_with(KEY, "s3cret-pass").unwrap();
        assert_ne!(ct, b"s3cret-pass");
        assert_eq!(decrypt_with(KEY, &ct).unwrap(), "s3cret-pass");
    }

    #[test]
    fn nonce_randomizes_ciphertext() {
        let a = encrypt_with(KEY, "x").unwrap();
        let b = encrypt_with(KEY, "x").unwrap();
        assert_ne!(a, b); // aynı düz metin farklı ciphertext (rastgele nonce)
    }

    #[test]
    fn bad_key_rejected() {
        assert!(matches!(encrypt_with("short", "x"), Err(CryptoError::BadKey)));
    }

    #[test]
    fn wrong_key_fails_decrypt() {
        let ct = encrypt_with(KEY, "x").unwrap();
        let other = "ZmVkY2JhOTg3NjU0MzIxMGZlZGNiYTk4NzY1NDMyMTA=";
        assert!(decrypt_with(other, &ct).is_err());
    }
}
```

- [ ] **Step 3: Test kırmızı→yeşil**

Run: `cargo test -p wf-wfe db::crypto 2>&1 | tail -8`
Expected: 4 test PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/src/lib.rs crates/wfe/src/db/mod.rs crates/wfe/src/db/crypto.rs
git commit -m "feat(wfe): db secret şifreleme (AES-256-GCM, saf test)"
```

---

## Task 4: Sürücü soyutlaması + test() — `mod.rs` + `drivers.rs`

**Files:** Modify `crates/wfe/src/db/mod.rs`, Create `crates/wfe/src/db/drivers.rs`

- [ ] **Step 1: `mod.rs`'e tipleri ekle**

`crates/wfe/src/db/mod.rs`'i şuna genişlet:

```rust
pub mod crypto;
pub mod drivers;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbDriver { Postgres, Mysql, Mssql }

impl DbDriver {
    pub fn parse(s: &str) -> Option<Self> {
        match s { "postgres" => Some(Self::Postgres), "mysql" => Some(Self::Mysql), "mssql" => Some(Self::Mssql), _ => None }
    }
}

/// Test/çalıştırma için çözülmüş (secret düz metin) bağlantı bilgisi.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub driver:   DbDriver,
    pub mode:     String,           // "fields" | "uri"
    pub host:     Option<String>,
    pub port:     Option<i32>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub secret:   Option<String>,   // parola (fields) veya bağlantı dizesi (uri)
    pub options:  serde_json::Value,
}

#[derive(Debug)]
pub struct DbError(pub String);
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl std::error::Error for DbError {}
```

- [ ] **Step 2: `drivers.rs` — bağlantı dizesi kurucular + test()**

`crates/wfe/src/db/drivers.rs`:

```rust
//! Sürücü-özel bağlantı testi. fields modu bileşenlerden URI kurar; uri modu
//! secret'ı doğrudan bağlantı dizesi olarak kullanır.
use super::{DbConfig, DbDriver, DbError};

fn field<'a>(o: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    o.get(k).and_then(|v| v.as_str())
}

/// fields modundan sqlx bağlantı dizesi (postgres/mysql).
fn sqlx_uri(cfg: &DbConfig, scheme: &str, default_port: i32) -> String {
    if cfg.mode == "uri" {
        return cfg.secret.clone().unwrap_or_default();
    }
    let host = cfg.host.as_deref().unwrap_or("localhost");
    let port = cfg.port.unwrap_or(default_port);
    let db = cfg.database.as_deref().unwrap_or("");
    let user = cfg.username.as_deref().unwrap_or("");
    let pass = cfg.secret.as_deref().unwrap_or("");
    format!("{scheme}://{user}:{pass}@{host}:{port}/{db}")
}

pub async fn test(cfg: &DbConfig) -> Result<(), DbError> {
    match cfg.driver {
        DbDriver::Postgres => test_sqlx_pg(cfg).await,
        DbDriver::Mysql => test_sqlx_my(cfg).await,
        DbDriver::Mssql => test_mssql(cfg).await,
    }
}

async fn test_sqlx_pg(cfg: &DbConfig) -> Result<(), DbError> {
    use sqlx::postgres::PgPoolOptions;
    let uri = sqlx_uri(cfg, "postgres", 5432);
    let pool = PgPoolOptions::new().max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(8))
        .connect(&uri).await.map_err(|e| DbError(e.to_string()))?;
    sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| DbError(e.to_string()))?;
    pool.close().await;
    Ok(())
}

async fn test_sqlx_my(cfg: &DbConfig) -> Result<(), DbError> {
    use sqlx::mysql::MySqlPoolOptions;
    let uri = sqlx_uri(cfg, "mysql", 3306);
    let pool = MySqlPoolOptions::new().max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(8))
        .connect(&uri).await.map_err(|e| DbError(e.to_string()))?;
    sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| DbError(e.to_string()))?;
    pool.close().await;
    Ok(())
}

async fn test_mssql(cfg: &DbConfig) -> Result<(), DbError> {
    use tiberius::{Config, AuthMethod};
    use tokio::net::TcpStream;
    use tokio_util::compat::TokioAsyncWriteCompatExt;

    let mut config = Config::new();
    if cfg.mode == "uri" {
        config = Config::from_ado_string(cfg.secret.as_deref().unwrap_or(""))
            .map_err(|e| DbError(e.to_string()))?;
    } else {
        config.host(cfg.host.as_deref().unwrap_or("localhost"));
        config.port(cfg.port.unwrap_or(1433) as u16);
        if let Some(db) = &cfg.database { config.database(db); }
        config.authentication(AuthMethod::sql_server(
            cfg.username.as_deref().unwrap_or(""),
            cfg.secret.as_deref().unwrap_or(""),
        ));
        if field(&cfg.options, "encrypt") == Some("false") {
            config.encryption(tiberius::EncryptionLevel::NotSupported);
        } else {
            config.trust_cert();
        }
    }
    let tcp = TcpStream::connect(config.get_addr()).await.map_err(|e| DbError(e.to_string()))?;
    tcp.set_nodelay(true).ok();
    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await
        .map_err(|e| DbError(e.to_string()))?;
    client.simple_query("SELECT 1").await.map_err(|e| DbError(e.to_string()))?;
    Ok(())
}
```

- [ ] **Step 3: Derleme**

Run: `cargo build -p wf-wfe 2>&1 | tail -8`
Expected: PASS. (`serde` wfe'de mevcut; değilse `crates/wfe/Cargo.toml`'a `serde = { workspace = true, features=["derive"] }` ekle.)

- [ ] **Step 4: Workspace derleme + crypto testleri**

Run: `cargo test -p wf-wfe 2>&1 | tail -8`
Expected: crypto testleri PASS; derleme temiz. (Sürücü `test()` canlı DB ister — Task 6'da curl ile doğrulanır.)

- [ ] **Step 5: Commit**

```bash
git add crates/wfe/src/db/mod.rs crates/wfe/src/db/drivers.rs crates/wfe/Cargo.toml
git commit -m "feat(wfe): DbDriver + üç sürücü test() soyutlaması (pg/mysql/mssql)"
```

---

## Task 5: `/db/connections` CRUD + test API

**Files:** Create `crates/server/src/routes/db.rs`, Modify `crates/server/src/main.rs`, `crates/server/src/routes/mod.rs`

- [ ] **Step 1: Route modülünü kaydet**

`crates/server/src/routes/mod.rs`'e ekle: `pub mod db;`

- [ ] **Step 2: `db.rs` handler'ları**

`crates/server/src/routes/db.rs`:

```rust
use crate::{error::AppError, state::AppState};
use axum::{extract::{Path, Query, State}, http::StatusCode, routing::{get, post}, Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;
use wf_wfe::db::{self, crypto, DbConfig, DbDriver};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/connections", get(list).post(create))
        .route("/connections/test", post(test_draft))
        .route("/connections/:id", axum::routing::put(update).delete(delete))
        .route("/connections/:id/test", post(test_saved))
        .with_state(state)
}

#[derive(Deserialize)]
struct TenantQuery { orgtnt_id: Uuid }

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
fn default_mode() -> String { "fields".into() }

fn to_config(b: &ConnBody, secret: Option<String>) -> Result<DbConfig, AppError> {
    let driver = DbDriver::parse(&b.driver)
        .ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::BAD_REQUEST))?;
    Ok(DbConfig {
        driver, mode: b.mode.clone(), host: b.host.clone(), port: b.port,
        database: b.database.clone(), username: b.username.clone(),
        secret, options: if b.options.is_null() { json!({}) } else { b.options.clone() },
    })
}

async fn list(State(s): State<AppState>, Query(q): Query<TenantQuery>)
    -> Result<Json<Value>, AppError>
{
    let rows = sqlx::query_as::<_, (Uuid, String, String, String, Option<String>, Option<i32>, Option<String>, Option<String>, Value, bool, Option<bool>, Option<chrono::DateTime<chrono::Utc>>)>(
        "SELECT id, name, driver, mode, host, port, database, username, options, is_active, last_test_ok, last_test_at \
         FROM wf.db_connection WHERE orgtnt_id=$1 AND is_active=true ORDER BY name")
        .bind(q.orgtnt_id).fetch_all(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    // secret ASLA dönmez
    let items: Vec<Value> = rows.into_iter().map(|r| json!({
        "id": r.0, "name": r.1, "driver": r.2, "mode": r.3, "host": r.4, "port": r.5,
        "database": r.6, "username": r.7, "options": r.8, "is_active": r.9,
        "last_test_ok": r.10, "last_test_at": r.11,
    })).collect();
    Ok(Json(json!(items)))
}

async fn create(State(s): State<AppState>, Json(b): Json<ConnBody>) -> Result<Json<Value>, AppError> {
    let orgtnt = b.orgtnt_id.ok_or_else(|| AppError("orgtnt_id gerekli".into(), StatusCode::BAD_REQUEST))?;
    let name = b.name.clone().ok_or_else(|| AppError("name gerekli".into(), StatusCode::BAD_REQUEST))?;
    let enc = match &b.secret {
        Some(sec) => Some(crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?),
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

async fn update(State(s): State<AppState>, Path(id): Path<Uuid>, Json(b): Json<ConnBody>)
    -> Result<StatusCode, AppError>
{
    // secret verilmezse mevcut korunur (COALESCE): None → NULL bind → COALESCE(NULL, secret_enc)
    let enc: Option<Vec<u8>> = match &b.secret {
        Some(sec) => Some(crypto::encrypt(sec).map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?),
        None => None,
    };
    let n = sqlx::query(
        "UPDATE wf.db_connection SET name=$2, driver=$3, mode=$4, host=$5, port=$6, database=$7, \
         username=$8, options=$9, secret_enc=COALESCE($10, secret_enc), updated_at=now() WHERE id=$1")
        .bind(id).bind(&b.name).bind(&b.driver).bind(&b.mode).bind(&b.host).bind(b.port)
        .bind(&b.database).bind(&b.username).bind(&b.options).bind(enc)
        .execute(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?.rows_affected();
    if n == 0 { return Err(AppError("bağlantı bulunamadı".into(), StatusCode::NOT_FOUND)); }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<StatusCode, AppError> {
    sqlx::query("DELETE FROM wf.db_connection WHERE id=$1").bind(id)
        .execute(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn test_draft(State(_s): State<AppState>, Json(b): Json<ConnBody>) -> Result<Json<Value>, AppError> {
    let cfg = to_config(&b, b.secret.clone())?;
    Ok(Json(run_test(&cfg).await))
}

async fn test_saved(State(s): State<AppState>, Path(id): Path<Uuid>) -> Result<Json<Value>, AppError> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, Option<i32>, Option<String>, Option<String>, Value, Option<Vec<u8>>)>(
        "SELECT driver, mode, host, port, database, username, options, secret_enc \
         FROM wf.db_connection WHERE id=$1")
        .bind(id).fetch_optional(&s.pool).await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?
        .ok_or_else(|| AppError("bağlantı bulunamadı".into(), StatusCode::NOT_FOUND))?;
    let driver = DbDriver::parse(&row.0).ok_or_else(|| AppError("geçersiz driver".into(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let secret = match row.7 {
        Some(bytes) => Some(crypto::decrypt(&bytes).map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?),
        None => None,
    };
    let cfg = DbConfig { driver, mode: row.1, host: row.2, port: row.3, database: row.4, username: row.5, secret, options: row.6 };
    let result = run_test(&cfg).await;
    let ok = result.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let _ = sqlx::query("UPDATE wf.db_connection SET last_test_at=now(), last_test_ok=$2 WHERE id=$1")
        .bind(id).bind(ok).execute(&s.pool).await;
    Ok(Json(result))
}

async fn run_test(cfg: &DbConfig) -> Value {
    match db::drivers::test(cfg).await {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "message": e.to_string() }),
    }
}
```

- [ ] **Step 3: `main.rs`'te `/db`'yi X-Admin-Key ile nest et**

`crates/server/src/main.rs`'te `/org` nest'inin hemen ardına, aynı admin-key middleware desenini `/db` için de uygula (org_router'ı üreten `match cfg.admin_api_key` bloğunu `db_router` için tekrarla veya ortak bir `guard(router)` yardımcıına çıkar). Minimal: `/org` guard'ını üreten kodu bir kapatıcıya alıp hem org hem db router'a uygula, sonra:

```rust
        .nest("/db", db_router)
```

`db_router = routes::db::router(state.clone())` (guard uygulanmış).

- [ ] **Step 4: Derleme + workspace test**

Run: `cargo build --workspace 2>&1 | tail -8`
Expected: PASS.
Run: `cargo test --workspace 2>&1 | grep -E "test result: FAILED" || echo OK`
Expected: OK (golden fixture bozulmaz).

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/routes/db.rs crates/server/src/routes/mod.rs crates/server/src/main.rs
git commit -m "feat(server): /db/connections CRUD + test API (X-Admin-Key korumalı)"
```

---

## Task 6: Uçtan uca manuel doğrulama (curl)

**Files:** yok.

Ön koşul: `.env`'e `DB_CONN_SECRET=<base64 32 byte>` ekle (ör. `openssl rand -base64 32`), `ADMIN_API_KEY` set, sunucuyu yeniden başlat. `$PORT`, `$TNT`, `$KEY=ADMIN_API_KEY`.

- [ ] **Step 1: Sunucu**

Run: `set -a; source .env; set +a; cargo run -p server` (arka planda; "listening" bekle).

- [ ] **Step 2: Postgres bağlantısı oluştur (kendi engine DB'sine)**

```bash
curl -s -X POST localhost:$PORT/db/connections -H "x-admin-key: $KEY" -H 'content-type: application/json' \
 -d "{\"orgtnt_id\":\"$TNT\",\"name\":\"local-pg\",\"driver\":\"postgres\",\"host\":\"localhost\",\"port\":5433,\"database\":\"wf_engine\",\"username\":\"apex\",\"secret\":\"<apex-parola>\"}"
```
Expected: `{"id":"<uuid>"}`. `CID`'ye al.

- [ ] **Step 3: Liste — secret DÖNMEZ**

Run: `curl -s "localhost:$PORT/db/connections?orgtnt_id=$TNT" -H "x-admin-key: $KEY"`
Expected: `local-pg` görünür; `secret`/`secret_enc` alanı YOK; `last_test_ok` baş null.

- [ ] **Step 4: Kayıtlı bağlantıyı test et**

Run: `curl -s -X POST localhost:$PORT/db/connections/$CID/test -H "x-admin-key: $KEY"`
Expected: `{"ok":true}`. Yanlış parolayla oluşturulan başka bir bağlantıda `{"ok":false,"message":"..."}`.

- [ ] **Step 5: Kaydetmeden test (draft)**

```bash
curl -s -X POST localhost:$PORT/db/connections/test -H "x-admin-key: $KEY" -H 'content-type: application/json' \
 -d "{\"driver\":\"postgres\",\"host\":\"localhost\",\"port\":5433,\"database\":\"wf_engine\",\"username\":\"apex\",\"secret\":\"<apex-parola>\"}"
```
Expected: `{"ok":true}`.

- [ ] **Step 6: Update secret korunur**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X PUT localhost:$PORT/db/connections/$CID -H "x-admin-key: $KEY" -H 'content-type: application/json' -d "{\"name\":\"local-pg\",\"driver\":\"postgres\",\"host\":\"localhost\",\"port\":5433,\"database\":\"wf_engine\",\"username\":\"apex\"}"`
Expected: `204`; ardından `/test` yine `{"ok":true}` (secret COALESCE ile korundu).

- [ ] **Step 7: Admin-key kapısı**

Run: `curl -s -o /dev/null -w '%{http_code}\n' "localhost:$PORT/db/connections?orgtnt_id=$TNT"` (key'siz)
Expected: `401`.

- [ ] **Step 8: MySQL/MSSQL (opsiyonel, DB varsa)**

Erişilebilir MySQL/MSSQL varsa aynı akışla `driver:"mysql"`/`"mssql"` test et; yoksa `{"ok":false,...}` bağlantı hatası mesajı beklenir (sürücü yolu çalışıyor demektir).

- [ ] **Step 9: Temizlik**

Run: `curl -s -o /dev/null -w '%{http_code}\n' -X DELETE localhost:$PORT/db/connections/$CID -H "x-admin-key: $KEY"` → `204`. Sunucuyu durdur.

Bu görevde commit yok.

---

## Self-Review Notları

- **Spec kapsamı (Faz 1a):** şema (Task 1) · şifreleme AES-GCM + write-only (Task 3, Task 5 list/create) · 3 sürücü test() (Task 4) · CRUD+test API (Task 5) · admin-key koruma (Task 5 Step 3) · secret update COALESCE (Task 5 update) — kapsandı.
- **Faz 1b (editör UI) ve Faz 2 (SQL node runtime bağlama)** bu planın DIŞINDA — ayrı planlar.
- **Tip tutarlılığı:** `DbConfig`/`DbDriver`/`db::drivers::test` Task 4'te tanımlı, Task 5'te aynı imzayla kullanılıyor; `crypto::encrypt/decrypt` Task 3 imzası Task 5'te tutarlı.
- **Kabul:** `tiberius` MSSQL için eklendi; şifreleme env `DB_CONN_SECRET`; secret API'de asla dönmez.
