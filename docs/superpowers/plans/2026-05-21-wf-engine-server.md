# WF Engine — Server Implementation Plan (Plan 2/2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> **Prerequisite:** Plan 1 (`2026-05-21-wf-engine-foundation.md`) must be complete and `cargo check --workspace` must pass before starting this plan.

**Goal:** Build the `wfd` crate (WFD storage adapter via OpenDAL), `wfe` crate (WFE persistence + execution orchestrator), and `server` crate (Axum HTTP binary) — producing a fully running workflow engine accessible via REST API.

**Architecture:** `wfd` implements `WfdPort` using OpenDAL (local fs / S3) for JSON storage + PostgreSQL for metadata. `wfe` implements `WfePort` using PostgreSQL (`wf` schema), and `OrgPort` using the `org` crate — making it the composition crate for org-to-engine bridging. `server` wires everything into a single Axum binary with `/org/*`, `/wfd/*`, `/wfe/*` route namespaces.

**Tech Stack:** Axum 0.7, sqlx 0.7, opendal 0.50, async-trait, tower-http (CORS), serde_json, tokio. Actor passed via three HTTP headers: `X-Actor-Orgu`, `X-Actor-User`, `X-Actor-Role`.

---

## File Map

```
crates/
├── wfd/
│   ├── Cargo.toml                              CREATE
│   └── src/
│       ├── lib.rs                              CREATE
│       ├── error.rs                            CREATE
│       ├── models.rs                           CREATE
│       ├── storage.rs                          CREATE
│       ├── repo.rs                             CREATE
│       └── adapter.rs                          CREATE
├── wfe/
│   ├── Cargo.toml                              CREATE
│   └── src/
│       ├── lib.rs                              CREATE
│       ├── error.rs                            CREATE
│       ├── models.rs                           CREATE
│       ├── repo/
│       │   ├── mod.rs                          CREATE
│       │   ├── wfe.rs                          CREATE
│       │   ├── dynctx.rs                       CREATE
│       │   └── wfah.rs                         CREATE
│       ├── org_adapter.rs                      CREATE  ← OrgPort impl using org crate
│       ├── wfe_adapter.rs                      CREATE  ← WfePort impl
│       └── executor.rs                         CREATE  ← WfeExecutor orchestrator
└── server/
    ├── Cargo.toml                              CREATE
    └── src/
        ├── main.rs                             CREATE
        ├── config.rs                           CREATE
        ├── error.rs                            CREATE
        ├── state.rs                            CREATE
        └── routes/
            ├── mod.rs                          CREATE
            ├── org.rs                          CREATE
            ├── wfd.rs                          CREATE
            └── wfe.rs                          CREATE
```

---

## Task 1: `wfd` Crate — Storage + Models + Repo

**Files:**
- Create: `crates/wfd/Cargo.toml`
- Create: `crates/wfd/src/lib.rs`
- Create: `crates/wfd/src/error.rs`
- Create: `crates/wfd/src/models.rs`
- Create: `crates/wfd/src/storage.rs`
- Create: `crates/wfd/src/repo.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
# crates/wfd/Cargo.toml
[package]
name    = "wf-wfd"
version = "0.1.0"
edition = "2021"

[dependencies]
wfe-core   = { path = "../wfe-core" }
sqlx       = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
uuid       = { workspace = true }
chrono     = { workspace = true }
opendal    = { workspace = true }
tracing    = { workspace = true }
async-trait = { workspace = true }
```

- [ ] **Step 2: Create `error.rs`**

```rust
// crates/wfd/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WfdError {
    #[error("wfd not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("invalid wfd json: {0}")]
    InvalidJson(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl From<WfdError> for wfe_core::EngineError {
    fn from(e: WfdError) -> Self {
        wfe_core::EngineError::WfdPort(e.to_string())
    }
}
```

- [ ] **Step 3: Create `models.rs`**

```rust
// crates/wfd/src/models.rs
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct WfdMeta {
    pub wfd_id:    Uuid,
    pub orgtnt_id: Uuid,
    pub name:      String,
    pub version:   i32,
    pub s3_key:    String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Create `storage.rs`**

```rust
// crates/wfd/src/storage.rs
use opendal::{Operator, services};

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    pub path:    String,           // local path
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StorageBackend {
    Local,
    S3,
}

impl StorageConfig {
    pub fn from_env() -> Self {
        let backend = match std::env::var("STORAGE_BACKEND")
            .unwrap_or_else(|_| "local".into())
            .as_str()
        {
            "s3" => StorageBackend::S3,
            _    => StorageBackend::Local,
        };
        Self {
            backend,
            path: std::env::var("STORAGE_PATH").unwrap_or_else(|_| "./storage".into()),
            s3_bucket: std::env::var("STORAGE_S3_BUCKET").ok(),
            s3_region: std::env::var("STORAGE_S3_REGION").ok(),
        }
    }
}

pub fn build_operator(cfg: &StorageConfig) -> Result<Operator, opendal::Error> {
    match cfg.backend {
        StorageBackend::Local => {
            let mut builder = services::Fs::default();
            builder.root(&cfg.path);
            Operator::new(builder)?.finish()
        }
        StorageBackend::S3 => {
            let mut builder = services::S3::default();
            builder.bucket(cfg.s3_bucket.as_deref().unwrap_or("wf-engine"));
            builder.region(cfg.s3_region.as_deref().unwrap_or("us-east-1"));
            Operator::new(builder)?.finish()
        }
    }
}

/// Canonical S3 key for a WFD JSON file.
pub fn s3_key(wfd_id: uuid::Uuid, version: i32) -> String {
    format!("wfd/{wfd_id}/{version}.json")
}
```

- [ ] **Step 5: Create `repo.rs`**

```rust
// crates/wfd/src/repo.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfdError, models::WfdMeta};

pub async fn insert(
    pool:      &PgPool,
    orgtnt_id: Uuid,
    name:      &str,
    version:   i32,
    s3_key:    &str,
) -> Result<Uuid, WfdError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfd_meta (orgtnt_id, name, version, s3_key)
         VALUES ($1, $2, $3, $4)
         RETURNING wfd_id"
    )
    .bind(orgtnt_id)
    .bind(name)
    .bind(version)
    .bind(s3_key)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

pub async fn get_meta(pool: &PgPool, wfd_id: Uuid, version: i32) -> Result<WfdMeta, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        "SELECT wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at
         FROM wf.wfd_meta
         WHERE wfd_id = $1 AND version = $2 AND is_active = true"
    )
    .bind(wfd_id)
    .bind(version)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfdError::NotFound(format!("{wfd_id} v{version}")))
}

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<WfdMeta>, WfdError> {
    sqlx::query_as::<_, WfdMeta>(
        "SELECT wfd_id, orgtnt_id, name, version, s3_key, is_active, created_at
         FROM wf.wfd_meta
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY name, version DESC"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(WfdError::Database)
}

pub async fn next_version(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<i32, WfdError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(version) FROM wf.wfd_meta WHERE orgtnt_id = $1 AND name = $2"
    )
    .bind(orgtnt_id)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
```

- [ ] **Step 6: Create `lib.rs`**

```rust
// crates/wfd/src/lib.rs
pub mod adapter;
pub mod error;
pub mod models;
pub mod repo;
pub mod storage;

pub use adapter::WfdAdapter;
pub use storage::{StorageConfig, build_operator};
```

- [ ] **Step 7: Create `adapter.rs` stub** (full impl in next task)

```rust
// crates/wfd/src/adapter.rs
use opendal::Operator;
use sqlx::PgPool;

pub struct WfdAdapter {
    pub pool:    PgPool,
    pub storage: Operator,
}

impl WfdAdapter {
    pub fn new(pool: PgPool, storage: Operator) -> Self {
        Self { pool, storage }
    }
}
```

- [ ] **Step 8: Verify compile**

```bash
cargo check -p wf-wfd 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 9: Commit**

```bash
git add crates/wfd/
git commit -m "feat: wfd crate — storage config, OpenDAL operator factory, wfd_meta repo"
```

---

## Task 2: `wfd` Crate — WfdAdapter (WfdPort impl)

**Files:**
- Modify: `crates/wfd/src/adapter.rs`

- [ ] **Step 1: Implement `WfdAdapter` as `WfdPort`**

```rust
// crates/wfd/src/adapter.rs
use async_trait::async_trait;
use opendal::Operator;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::{EngineError, WfdPort, types::wfd::WFD};
use crate::{repo, storage};

pub struct WfdAdapter {
    pub pool:    PgPool,
    pub storage: Operator,
}

impl WfdAdapter {
    pub fn new(pool: PgPool, storage: Operator) -> Self {
        Self { pool, storage }
    }

    /// Upload a new WFD — stores JSON in OpenDAL, metadata in PostgreSQL.
    pub async fn upload(
        &self,
        orgtnt_id: Uuid,
        wfd:       &WFD,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let version = repo::next_version(&self.pool, orgtnt_id, &wfd.name).await?;
        let wfd_id  = uuid::Uuid::new_v4();
        let key     = storage::s3_key(wfd_id, version);

        let bytes = serde_json::to_vec(wfd)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage.write(&key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(&self.pool, orgtnt_id, &wfd.name, version, &key).await?;
        Ok((wfd_id, version))
    }
}

#[async_trait]
impl WfdPort for WfdAdapter {
    async fn fetch(&self, wfd_id: Uuid, version: u32) -> Result<WFD, EngineError> {
        let meta = repo::get_meta(&self.pool, wfd_id, version as i32)
            .await
            .map_err(|e| EngineError::WfdPort(e.to_string()))?;

        let bytes = self.storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| EngineError::WfdPort(format!("storage read: {e}")))?
            .to_bytes();

        serde_json::from_slice::<WFD>(&bytes)
            .map_err(|e| EngineError::InvalidWfd(e.to_string()))
    }
}
```

- [ ] **Step 2: Verify compile**

```bash
cargo check -p wf-wfd 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/wfd/src/adapter.rs
git commit -m "feat: wfd WfdAdapter implements WfdPort via OpenDAL + PostgreSQL"
```

---

## Task 3: `wfe` Crate — Repos + Models

**Files:**
- Create: `crates/wfe/Cargo.toml`
- Create: `crates/wfe/src/lib.rs`
- Create: `crates/wfe/src/error.rs`
- Create: `crates/wfe/src/models.rs`
- Create: `crates/wfe/src/repo/mod.rs`
- Create: `crates/wfe/src/repo/wfe.rs`
- Create: `crates/wfe/src/repo/dynctx.rs`
- Create: `crates/wfe/src/repo/wfah.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
# crates/wfe/Cargo.toml
[package]
name    = "wf-wfe"
version = "0.1.0"
edition = "2021"

[dependencies]
wfe-core   = { path = "../wfe-core" }
wf-org     = { path = "../org" }
sqlx       = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
uuid       = { workspace = true }
chrono     = { workspace = true }
async-trait = { workspace = true }
tracing    = { workspace = true }
```

- [ ] **Step 2: Create `error.rs`**

```rust
// crates/wfe/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WfeError {
    #[error("wfe not found: {0}")]
    NotFound(String),
    #[error("wfe is terminal")]
    Terminal,
    #[error(transparent)]
    Engine(#[from] wfe_core::EngineError),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
```

- [ ] **Step 3: Create `models.rs`**

```rust
// crates/wfe/src/models.rs
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow)]
pub struct WfeRow {
    pub wfe_id:       Uuid,
    pub orgtnt_id:    Uuid,
    pub wfd_id:       Uuid,
    pub wfd_version:  i32,
    pub status:       String,
    pub current_c_a:  serde_json::Value,
    pub end_response: Option<serde_json::Value>,
    pub created_at:   DateTime<Utc>,
    pub updated_at:   DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct DynCtxRow {
    pub dynctx_id:  Uuid,
    pub wfe_id:     Uuid,
    pub seq:        i32,
    pub ctx:        serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct WfahRow {
    pub wfah_id:    Uuid,
    pub wfe_id:     Uuid,
    pub seq:        i32,
    pub action:     String,
    pub actor:      serde_json::Value,
    pub input:      Option<serde_json::Value>,
    pub applied_at: DateTime<Utc>,
}
```

- [ ] **Step 4: Create `repo/wfe.rs`**

```rust
// crates/wfe/src/repo/wfe.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfeError, models::WfeRow};

pub async fn create(
    pool:        &PgPool,
    orgtnt_id:   Uuid,
    wfd_id:      Uuid,
    wfd_version: i32,
    current_c_a: &serde_json::Value,
) -> Result<Uuid, WfeError> {
    let wfe_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO wf.wfe (orgtnt_id, wfd_id, wfd_version, status, current_c_a)
         VALUES ($1, $2, $3, 'active', $4)
         RETURNING wfe_id"
    )
    .bind(orgtnt_id)
    .bind(wfd_id)
    .bind(wfd_version)
    .bind(current_c_a)
    .fetch_one(pool)
    .await?;
    Ok(wfe_id)
}

pub async fn get(pool: &PgPool, wfe_id: Uuid) -> Result<WfeRow, WfeError> {
    sqlx::query_as::<_, WfeRow>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, status,
                current_c_a, end_response, created_at, updated_at
         FROM wf.wfe WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(wfe_id.to_string()))
}

pub async fn update_c_a(
    pool:    &PgPool,
    wfe_id:  Uuid,
    c_a:     &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "UPDATE wf.wfe SET current_c_a = $1, updated_at = now() WHERE wfe_id = $2"
    )
    .bind(c_a)
    .bind(wfe_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_terminal(
    pool:         &PgPool,
    wfe_id:       Uuid,
    end_response: &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "UPDATE wf.wfe
         SET status = 'terminal', end_response = $1, updated_at = now()
         WHERE wfe_id = $2"
    )
    .bind(end_response)
    .bind(wfe_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<WfeRow>, WfeError> {
    sqlx::query_as::<_, WfeRow>(
        "SELECT wfe_id, orgtnt_id, wfd_id, wfd_version, status,
                current_c_a, end_response, created_at, updated_at
         FROM wf.wfe WHERE orgtnt_id = $1 ORDER BY created_at DESC"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
```

- [ ] **Step 5: Create `repo/dynctx.rs`**

```rust
// crates/wfe/src/repo/dynctx.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::error::WfeError;

/// Insert a new DynCtx snapshot. Insert-only — never update existing rows.
pub async fn insert(
    pool:   &PgPool,
    wfe_id: Uuid,
    seq:    i32,
    ctx:    &serde_json::Value,
) -> Result<(), WfeError> {
    sqlx::query(
        "INSERT INTO wf.wfe_dynctx (wfe_id, seq, ctx) VALUES ($1, $2, $3)"
    )
    .bind(wfe_id)
    .bind(seq)
    .bind(ctx)
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns the latest DynCtx snapshot for a WFE.
pub async fn load_latest(
    pool:   &PgPool,
    wfe_id: Uuid,
) -> Result<serde_json::Value, WfeError> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT ctx FROM wf.wfe_dynctx
         WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1"
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(format!("dynctx for wfe {wfe_id}")))
}

pub async fn next_seq(pool: &PgPool, wfe_id: Uuid) -> Result<i32, WfeError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(seq) FROM wf.wfe_dynctx WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
```

- [ ] **Step 6: Create `repo/wfah.rs`**

```rust
// crates/wfe/src/repo/wfah.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::WfeError, models::WfahRow};

pub async fn append(
    pool:   &PgPool,
    wfe_id: Uuid,
    seq:    i32,
    action: &str,
    actor:  &serde_json::Value,
    input:  Option<&serde_json::Value>,
) -> Result<(), WfeError> {
    sqlx::query(
        "INSERT INTO wf.wfah (wfe_id, seq, action, actor, input)
         VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(wfe_id)
    .bind(seq)
    .bind(action)
    .bind(actor)
    .bind(input)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<WfahRow>, WfeError> {
    sqlx::query_as::<_, WfahRow>(
        "SELECT wfah_id, wfe_id, seq, action, actor, input, applied_at
         FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq ASC"
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

pub async fn next_seq(pool: &PgPool, wfe_id: Uuid) -> Result<i32, WfeError> {
    let max: Option<i32> = sqlx::query_scalar(
        "SELECT MAX(seq) FROM wf.wfah WHERE wfe_id = $1"
    )
    .bind(wfe_id)
    .fetch_one(pool)
    .await?;
    Ok(max.unwrap_or(0) + 1)
}
```

- [ ] **Step 7: Create `repo/mod.rs` and `lib.rs`**

```rust
// crates/wfe/src/repo/mod.rs
pub mod dynctx;
pub mod wfah;
pub mod wfe;
```

```rust
// crates/wfe/src/lib.rs
pub mod error;
pub mod executor;
pub mod models;
pub mod org_adapter;
pub mod repo;
pub mod wfe_adapter;

pub use executor::WfeExecutor;
pub use org_adapter::OrgAdapter;
pub use wfe_adapter::WfeAdapter;
```

- [ ] **Step 8: Create stubs for adapter files**

```rust
// crates/wfe/src/org_adapter.rs
use sqlx::PgPool;
pub struct OrgAdapter { pub pool: PgPool }
impl OrgAdapter { pub fn new(pool: PgPool) -> Self { Self { pool } } }
```

```rust
// crates/wfe/src/wfe_adapter.rs
use sqlx::PgPool;
pub struct WfeAdapter { pub pool: PgPool }
impl WfeAdapter { pub fn new(pool: PgPool) -> Self { Self { pool } } }
```

```rust
// crates/wfe/src/executor.rs
```

- [ ] **Step 9: Verify compile**

```bash
cargo check -p wf-wfe 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 10: Commit**

```bash
git add crates/wfe/
git commit -m "feat: wfe crate — DB repos for wfe/dynctx/wfah, insert-only dynctx guarantee"
```

---

## Task 4: `wfe` Crate — OrgAdapter + WfeAdapter

**Files:**
- Modify: `crates/wfe/src/org_adapter.rs`
- Modify: `crates/wfe/src/wfe_adapter.rs`

- [ ] **Step 1: Implement `OrgAdapter` as `OrgPort`**

`OrgAdapter` bridges the `org` crate's `user_role::resolve_orgu` and `user_role::check_user_role` functions to the `OrgPort` trait defined in `wfe-core`.

```rust
// crates/wfe/src/org_adapter.rs
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use wf_org::repo::user_role;
use wfe_core::{EngineError, OrgPort, types::actor::OrgUnit};

pub struct OrgAdapter {
    pub pool: PgPool,
}

impl OrgAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OrgPort for OrgAdapter {
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr:           &str,
        orgtnt_id:      Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        let units = user_role::resolve_orgu(&self.pool, anchor_orgu_id, expr, orgtnt_id)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))?;

        Ok(units.into_iter().map(|u| OrgUnit {
            orgu_id:   u.orgu_id,
            orgu_type: u.orgu_type,
            path:      u.path,
        }).collect())
    }

    async fn check_user_role(
        &self,
        user_id:   Uuid,
        orgu_id:   Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError> {
        user_role::check_user_role(&self.pool, user_id, orgu_id, role_name)
            .await
            .map_err(|e| EngineError::OrgPort(e.to_string()))
    }
}
```

- [ ] **Step 2: Implement `WfeAdapter` as `WfePort`**

```rust
// crates/wfe/src/wfe_adapter.rs
use async_trait::async_trait;
use sqlx::PgPool;
use uuid::Uuid;
use serde_json::{json, Value};
use wfe_core::{
    EngineError, WfePort,
    ports::WFES,
    types::{
        actor::{Actor, CandidateActor},
        dynctx::DynCtx,
        wfah::{Wfah, WfahEntry},
        wfe::WfeStatus,
    },
};
use crate::repo;

pub struct WfeAdapter {
    pub pool: PgPool,
}

impl WfeAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl WfePort for WfeAdapter {
    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError> {
        let row = repo::wfe::get(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let ctx_val = repo::dynctx::load_latest(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let wfah_rows = repo::wfah::load_all(&self.pool, wfe_id)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        let entries: Vec<WfahEntry> = wfah_rows
            .into_iter()
            .map(|r| {
                let actor: Actor = serde_json::from_value(r.actor)
                    .map_err(|e| EngineError::WfePort(e.to_string()))
                    .unwrap_or(Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role:    "unknown".into(),
                    });
                WfahEntry {
                    seq:        r.seq as u32,
                    action:     r.action,
                    actor,
                    input:      r.input,
                    applied_at: r.applied_at,
                }
            })
            .collect();

        let status = match row.status.as_str() {
            "terminal" => WfeStatus::Terminal,
            "error"    => WfeStatus::Error,
            _          => WfeStatus::Active,
        };

        Ok(WFES {
            wfe_id,
            dynctx:      DynCtx(ctx_val),
            wfah:        Wfah(entries),
            status,
            orgtnt_id:   row.orgtnt_id,
            wfd_id:      row.wfd_id,
            wfd_version: row.wfd_version as u32,
        })
    }

    async fn persist_new_dynctx(
        &self,
        wfe_id: Uuid,
        ctx:    &DynCtx,
        seq:    u32,
    ) -> Result<(), EngineError> {
        repo::dynctx::insert(&self.pool, wfe_id, seq as i32, ctx.as_value())
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn append_wfah(
        &self,
        wfe_id: Uuid,
        entry:  &WfahEntry,
    ) -> Result<(), EngineError> {
        let actor_json = serde_json::to_value(&entry.actor)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        repo::wfah::append(
            &self.pool, wfe_id, entry.seq as i32,
            &entry.action, &actor_json, entry.input.as_ref(),
        )
        .await
        .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn update_c_a(
        &self,
        wfe_id: Uuid,
        c_a:    &[CandidateActor],
    ) -> Result<(), EngineError> {
        let c_a_json = serde_json::to_value(c_a)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        repo::wfe::update_c_a(&self.pool, wfe_id, &c_a_json)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn set_terminal(
        &self,
        wfe_id:       Uuid,
        end_response: &Value,
    ) -> Result<(), EngineError> {
        repo::wfe::set_terminal(&self.pool, wfe_id, end_response)
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))
    }

    async fn create_wfe(
        &self,
        orgtnt_id:   Uuid,
        wfd_id:      Uuid,
        wfd_version: u32,
        initial_ctx: &DynCtx,
        initial_c_a: &[CandidateActor],
    ) -> Result<Uuid, EngineError> {
        let c_a_json = serde_json::to_value(initial_c_a)
            .map_err(|e| EngineError::WfePort(e.to_string()))?;
        let wfe_id = repo::wfe::create(
            &self.pool, orgtnt_id, wfd_id, wfd_version as i32, &c_a_json
        )
        .await
        .map_err(|e| EngineError::WfePort(e.to_string()))?;

        // Persist initial DynCtx as seq=1
        repo::dynctx::insert(&self.pool, wfe_id, 1, initial_ctx.as_value())
            .await
            .map_err(|e| EngineError::WfePort(e.to_string()))?;

        Ok(wfe_id)
    }
}
```

- [ ] **Step 3: Verify compile**

```bash
cargo check -p wf-wfe 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/src/org_adapter.rs crates/wfe/src/wfe_adapter.rs
git commit -m "feat: wfe OrgAdapter (OrgPort impl) and WfeAdapter (WfePort impl)"
```

---

## Task 5: `wfe` Crate — WfeExecutor

**Files:**
- Modify: `crates/wfe/src/executor.rs`

- [ ] **Step 1: Implement `WfeExecutor`**

```rust
// crates/wfe/src/executor.rs
use std::sync::Arc;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::{
    engine::transition::{apply_action, WftOutcome},
    engine::c_a_resolver::resolve_c_a,
    engine::visibility,
    error::EngineError,
    ports::{OrgPort, WfdPort, WfePort, WFES},
    types::{
        actor::{Actor, CandidateActor},
        dynctx::DynCtx,
        wfah::Wfah,
        wfd::WFD,
        wfe::WfeStatus,
    },
    zen,
};

pub struct WfeExecutor {
    pub org: Arc<dyn OrgPort>,
    pub wfd: Arc<dyn WfdPort>,
    pub wfe: Arc<dyn WfePort>,
}

impl WfeExecutor {
    pub fn new(
        org: Arc<dyn OrgPort>,
        wfd: Arc<dyn WfdPort>,
        wfe: Arc<dyn WfePort>,
    ) -> Self {
        Self { org, wfd, wfe }
    }
}

/// Response from WfeExecutor::start
#[derive(Debug, serde::Serialize)]
pub struct WfeStartResult {
    pub wfe_id:      Uuid,
    pub current_c_a: Vec<CandidateActor>,
}

/// Response from WfeExecutor::apply
#[derive(Debug, serde::Serialize)]
pub struct WfeApplyResult {
    pub wfe_id:   Uuid,
    pub terminal: bool,
    pub end_response: Option<Value>,
    pub current_c_a:  Vec<CandidateActor>,
}

/// Response from WfeExecutor::query — DynCtx filtered by V(DynCtx, viewer)
#[derive(Debug, serde::Serialize)]
pub struct WfeView {
    pub wfe_id:      Uuid,
    pub status:      WfeStatus,
    pub dynctx:      Value,
    pub wfah:        Vec<wfe_core::types::wfah::WfahEntry>,
    pub current_c_a: Value,
    pub end_response: Option<Value>,
}

impl WfeExecutor {
    /// Start a new WFE instance.
    pub async fn start(
        &self,
        wfd_id:    Uuid,
        version:   u32,
        actor:     &Actor,
        input:     &Value,
    ) -> Result<WfeStartResult, EngineError> {
        let wfd = self.wfd.fetch(wfd_id, version).await?;

        // Find matching start rule — actor must be in c_a
        let start_rule = 'outer: {
            for rule in &wfd.start {
                for ca_rule in &rule.c_a {
                    let orgus = self.org
                        .resolve_c_orgu(actor.orgu_id, &ca_orgu_expr(ca_rule), actor.orgu_id)
                        .await?;
                    let in_orgu = orgus.iter().any(|u| u.orgu_id == actor.orgu_id);
                    if in_orgu && self.org.check_user_role(actor.user_id, actor.orgu_id, &actor.role).await? {
                        break 'outer Some(rule);
                    }
                }
            }
            None
        };

        let start_rule = start_rule.ok_or(EngineError::StartNotEligible)?;

        // Build initial DynCtx by applying start wfes_effects
        let initial_dynctx = DynCtx::empty();
        // Use a temporary wfe_id for effect resolution — replaced after create_wfe
        let temp_wfe_id = Uuid::new_v4();
        let initial_dynctx = wfe_core::engine::dynctx_apply::apply(
            &initial_dynctx, &start_rule.wfes_effects, actor, temp_wfe_id, "start", input
        )?;

        // Resolve initial C_A from start.wft
        let temp_wfes = WFES {
            wfe_id: temp_wfe_id,
            dynctx: initial_dynctx.clone(),
            wfah: Wfah::empty(),
            status: WfeStatus::Active,
            orgtnt_id: Uuid::nil(), // filled after create
            wfd_id,
            wfd_version: version,
        };
        let initial_c_a = resolve_wft_c_a(&start_rule.wft, &temp_wfes, actor.orgu_id, &*self.org).await?;

        // Persist new WFE (wfe + dynctx seq=1)
        let wfe_id = self.wfe.create_wfe(
            actor.orgu_id, // use actor's orgtnt — caller provides this via orgtnt_id field
            wfd_id, version, &initial_dynctx, &initial_c_a,
        ).await?;

        // Append "start" to WFAH seq=1
        let entry = wfe_core::types::wfah::WfahEntry {
            seq:        1,
            action:     "start".into(),
            actor:      actor.clone(),
            input:      Some(input.clone()),
            applied_at: chrono::Utc::now(),
        };
        self.wfe.append_wfah(wfe_id, &entry).await?;

        Ok(WfeStartResult { wfe_id, current_c_a: initial_c_a })
    }

    /// Apply an action to an existing WFE.
    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
        action: &str,
        input:  &Value,
    ) -> Result<WfeApplyResult, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;

        if wfes.status == WfeStatus::Terminal {
            return Err(EngineError::WfeTerminal);
        }

        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        let (new_wfes, outcome) = apply_action(&wfes, actor, action, input, &wfd, &*self.org).await?;

        // Next seq numbers
        let dynctx_seq = (new_wfes.wfah.entries().len() as u32); // same seq as wfah
        let wfah_seq   = dynctx_seq;

        // Persist new DynCtx snapshot (insert-only)
        self.wfe.persist_new_dynctx(wfe_id, &new_wfes.dynctx, dynctx_seq).await?;

        // Append WFAH entry
        if let Some(entry) = new_wfes.wfah.entries().last() {
            self.wfe.append_wfah(wfe_id, entry).await?;
        }

        match outcome {
            WftOutcome::Terminal { end_response } => {
                self.wfe.set_terminal(wfe_id, &end_response).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal: true,
                    end_response: Some(end_response),
                    current_c_a: vec![],
                })
            }
            WftOutcome::NextCa(c_a) => {
                self.wfe.update_c_a(wfe_id, &c_a).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal: false,
                    end_response: None,
                    current_c_a: c_a,
                })
            }
        }
    }

    /// Query WFE state with visibility filtering applied.
    pub async fn query(
        &self,
        wfe_id: Uuid,
        viewer: &Actor,
    ) -> Result<WfeView, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        let wfd  = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let filtered_ctx = visibility::apply(&wfes.dynctx, viewer, &wfd);

        let row = crate::repo::wfe::get(
            // Note: need pool access here — pass via a method or store pool in executor
            // For now we use current_c_a from wfes struct (loaded by WfePort)
            // See note below about pool access
            &sqlx::PgPool::connect("").await.unwrap_or_else(|_| unreachable!()),
            wfe_id,
        ).await;

        // WfePort::load_wfes already gives us what we need. Build WfeView directly:
        Ok(WfeView {
            wfe_id,
            status: wfes.status,
            dynctx: filtered_ctx,
            wfah: wfes.wfah.entries().to_vec(),
            current_c_a: serde_json::Value::Null,  // filled by caller from wfe row
            end_response: None,
        })
    }

    /// Returns action names the actor can perform on this WFE right now.
    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
    ) -> Result<Vec<String>, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        if wfes.status == WfeStatus::Terminal {
            return Ok(vec![]);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        let mut actions = Vec::new();
        for t in &wfd.transitions {
            if zen::evaluate(&t.when, &wfes)? {
                if wfe_core::engine::c_a_resolver::actor_in_c_a(
                    &t.c_a, actor, wfes.orgtnt_id, &*self.org
                ).await? {
                    actions.push(t.action.clone());
                }
            }
        }
        actions.dedup();
        Ok(actions)
    }
}

fn ca_orgu_expr(rule: &wfe_core::types::actor::CaRule) -> String {
    match &rule.c_orgu {
        wfe_core::types::actor::COrguExpr::Expr(s) => s.clone(),
        wfe_core::types::actor::COrguExpr::Anchored { traverse, .. } => traverse.clone(),
    }
}

async fn resolve_wft_c_a(
    wft:    &wfe_core::types::wfd::WftRule,
    wfes:   &WFES,
    anchor: Uuid,
    org:    &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    use wfe_core::types::wfd::WftRule;
    match wft {
        WftRule::Simple { c_a } => {
            resolve_c_a(c_a, anchor, wfes.orgtnt_id, org).await
        }
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if !cond.terminal {
                    if let Some(c_a) = &cond.c_a {
                        return resolve_c_a(c_a, anchor, wfes.orgtnt_id, org).await;
                    }
                }
            }
            Ok(vec![])
        }
    }
}
```

> **Note about `query`:** The `WfeView.current_c_a` and `end_response` fields need the raw WFE row. Refactor `WfePort` to include a `get_wfe_row` method OR extend `WFES` struct to carry `current_c_a` and `end_response`. The simplest fix: add `current_c_a: Vec<CandidateActor>` and `end_response: Option<Value>` to `WFES` struct in `wfe-core/src/ports.rs`, populate them in `WfeAdapter::load_wfes`.

- [ ] **Step 2: Apply the WFES fix — add current_c_a and end_response to WFES**

In `crates/wfe-core/src/ports.rs`, extend `WFES`:

```rust
// In ports.rs, add two fields to WFES:
pub struct WFES {
    pub wfe_id:       Uuid,
    pub dynctx:       DynCtx,
    pub wfah:         crate::types::wfah::Wfah,
    pub status:       WfeStatus,
    pub orgtnt_id:    Uuid,
    pub wfd_id:       Uuid,
    pub wfd_version:  u32,
    pub current_c_a:  Vec<crate::types::actor::CandidateActor>,  // ADD
    pub end_response: Option<serde_json::Value>,                  // ADD
}
```

Update `WfeAdapter::load_wfes` to populate them:

```rust
// In wfe_adapter.rs load_wfes, after building entries:
let current_c_a: Vec<CandidateActor> = serde_json::from_value(row.current_c_a.clone())
    .unwrap_or_default();

Ok(WFES {
    wfe_id,
    dynctx: DynCtx(ctx_val),
    wfah: Wfah(entries),
    status,
    orgtnt_id: row.orgtnt_id,
    wfd_id: row.wfd_id,
    wfd_version: row.wfd_version as u32,
    current_c_a,
    end_response: row.end_response,
})
```

Simplify `query` in `executor.rs`:

```rust
pub async fn query(&self, wfe_id: Uuid, viewer: &Actor) -> Result<WfeView, EngineError> {
    let wfes = self.wfe.load_wfes(wfe_id).await?;
    let wfd  = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
    let filtered_ctx = visibility::apply(&wfes.dynctx, viewer, &wfd);

    Ok(WfeView {
        wfe_id,
        status:       wfes.status.clone(),
        dynctx:       filtered_ctx,
        wfah:         wfes.wfah.entries().to_vec(),
        current_c_a:  serde_json::to_value(&wfes.current_c_a).unwrap_or_default(),
        end_response: wfes.end_response.clone(),
    })
}
```

- [ ] **Step 3: Verify compile**

```bash
cargo check -p wf-wfe 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/wfe/ crates/wfe-core/src/ports.rs
git commit -m "feat: wfe executor — start/apply/query/possible-actions orchestration"
```

---

## Task 6: `server` Crate — Config + State + Error + Main Skeleton

**Files:**
- Create: `crates/server/Cargo.toml`
- Create: `crates/server/src/config.rs`
- Create: `crates/server/src/error.rs`
- Create: `crates/server/src/state.rs`
- Create: `crates/server/src/main.rs`
- Create: `crates/server/src/routes/mod.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
# crates/server/Cargo.toml
[package]
name    = "wf-server"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "wf-server"
path = "src/main.rs"

[dependencies]
wfe-core   = { path = "../wfe-core" }
wf-org     = { path = "../org" }
wf-wfd     = { path = "../wfd" }
wf-wfe     = { path = "../wfe" }
axum       = { workspace = true }
tokio      = { workspace = true }
sqlx       = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
uuid       = { workspace = true }
chrono     = { workspace = true }
tower-http = { workspace = true }
tracing    = { workspace = true }
tracing-subscriber = { workspace = true }
dotenvy    = { workspace = true }
```

- [ ] **Step 2: Create `config.rs`**

```rust
// crates/server/src/config.rs
#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port:         u16,
    pub storage:      wf_wfd::StorageConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        dotenvy::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL not set")?,
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .map_err(|_| "PORT must be a number")?,
            storage: wf_wfd::StorageConfig::from_env(),
        })
    }
}
```

- [ ] **Step 3: Create `error.rs`**

```rust
// crates/server/src/error.rs
use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;
use wfe_core::EngineError;

#[derive(Debug)]
pub struct AppError(pub String, pub StatusCode);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.1, Json(json!({"error": self.0}))).into_response()
    }
}

impl From<EngineError> for AppError {
    fn from(e: EngineError) -> Self {
        let status = match &e {
            EngineError::PermissionDenied(_)   => StatusCode::FORBIDDEN,
            EngineError::TransitionNotFound(_) => StatusCode::BAD_REQUEST,
            EngineError::WfeTerminal           => StatusCode::CONFLICT,
            EngineError::StartNotEligible      => StatusCode::FORBIDDEN,
            _                                  => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(e.to_string(), status)
    }
}

impl From<wf_org::error::OrgError> for AppError {
    fn from(e: wf_org::error::OrgError) -> Self {
        let status = match &e {
            wf_org::error::OrgError::NotFound(_)   => StatusCode::NOT_FOUND,
            wf_org::error::OrgError::BadRequest(_)  => StatusCode::BAD_REQUEST,
            _                                       => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(e.to_string(), status)
    }
}
```

- [ ] **Step 4: Create `state.rs`**

```rust
// crates/server/src/state.rs
use std::sync::Arc;
use sqlx::PgPool;
use wf_wfe::WfeExecutor;
use wf_wfd::WfdAdapter;

#[derive(Clone)]
pub struct AppState {
    pub pool:     PgPool,
    pub executor: Arc<WfeExecutor>,
    pub wfd:      Arc<WfdAdapter>,
}
```

- [ ] **Step 5: Create `routes/mod.rs`**

```rust
// crates/server/src/routes/mod.rs
pub mod org;
pub mod wfd;
pub mod wfe;
```

- [ ] **Step 6: Create `main.rs` skeleton**

```rust
// crates/server/src/main.rs
mod config;
mod error;
mod routes;
mod state;

use std::sync::Arc;
use axum::Router;
use sqlx::postgres::PgPoolOptions;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use wf_wfe::{OrgAdapter, WfeAdapter, WfeExecutor};

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let cfg = config::Config::from_env().expect("config error");

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&cfg.database_url)
        .await
        .expect("db connect failed");

    let storage = wf_wfd::build_operator(&cfg.storage).expect("storage init failed");

    let org_adapter = Arc::new(OrgAdapter::new(pool.clone()));
    let wfd_adapter = Arc::new(wf_wfd::WfdAdapter::new(pool.clone(), storage));
    let wfe_adapter = Arc::new(WfeAdapter::new(pool.clone()));

    let executor = Arc::new(WfeExecutor::new(
        org_adapter.clone(),
        wfd_adapter.clone(),
        wfe_adapter,
    ));

    let state = state::AppState {
        pool: pool.clone(),
        executor,
        wfd: wfd_adapter,
    };

    let app = Router::new()
        .nest("/org", routes::org::router(pool.clone()))
        .nest("/wfd", routes::wfd::router(state.clone()))
        .nest("/wfe", routes::wfe::router(state.clone()))
        .layer(CorsLayer::permissive());

    let addr = format!("0.0.0.0:{}", cfg.port);
    tracing::info!("listening on {addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

- [ ] **Step 7: Verify compile with stub route files**

Create empty stub route files so `mod` declarations resolve:

```bash
for f in org wfd wfe; do
cat > crates/server/src/routes/${f}.rs << 'EOF'
use axum::Router;
pub fn router<S: Clone + Send + Sync + 'static>(_state: S) -> Router { Router::new() }
EOF
done

# org router takes PgPool directly
cat > crates/server/src/routes/org.rs << 'EOF'
use axum::Router;
use sqlx::PgPool;
pub fn router(_pool: PgPool) -> Router { Router::new() }
EOF
```

```bash
cargo check -p wf-server 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add crates/server/
git commit -m "feat: server crate — config, state, error, main skeleton"
```

---

## Task 7: `server` Crate — Org Routes

**Files:**
- Modify: `crates/server/src/routes/org.rs`

- [ ] **Step 1: Implement org routes**

```rust
// crates/server/src/routes/org.rs
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
        .route("/orgtnt",              get(list_orgtnt))
        .route("/orgtnt/:id",          get(get_orgtnt))
        .route("/orgtnt/:id/orgt",     get(list_orgt_by_tenant))
        .route("/orgt/:id/orgu",       get(list_orgu_by_tree))
        .route("/orgu/:id",            get(get_orgu))
        .route("/orgu/:id/traverse",   get(traverse_orgu))
        .with_state(pool)
}

async fn list_orgtnt(State(pool): State<PgPool>) -> Result<Json<Vec<wf_org::models::Orgtnt>>, AppError> {
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
```

- [ ] **Step 2: Verify compile**

```bash
cargo check -p wf-server 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/routes/org.rs
git commit -m "feat: server org routes — /org/orgtnt, /org/orgt, /org/orgu, /org/orgu/:id/traverse"
```

---

## Task 8: `server` Crate — WFD + WFE Routes

**Files:**
- Modify: `crates/server/src/routes/wfd.rs`
- Modify: `crates/server/src/routes/wfe.rs`

- [ ] **Step 1: Implement wfd routes**

```rust
// crates/server/src/routes/wfd.rs
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use uuid::Uuid;
use wfe_core::types::wfd::WFD;
use crate::{error::AppError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",             post(upload_wfd).get(list_wfd))
        .route("/:id/:version", get(get_wfd))
        .with_state(state)
}

#[derive(Deserialize)]
struct ListQuery { orgtnt_id: Uuid }

async fn list_wfd(
    State(s): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<wf_wfd::models::WfdMeta>>, AppError> {
    wf_wfd::repo::list(&s.pool, q.orgtnt_id)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}

#[derive(Deserialize)]
struct UploadBody {
    orgtnt_id: Uuid,
    #[serde(flatten)]
    wfd: WFD,
}

async fn upload_wfd(
    State(s): State<AppState>,
    Json(body): Json<UploadBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let (wfd_id, version) = s.wfd.upload(body.orgtnt_id, &body.wfd)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(serde_json::json!({ "wfd_id": wfd_id, "version": version })))
}

async fn get_wfd(
    State(s): State<AppState>,
    Path((wfd_id, version)): Path<(Uuid, u32)>,
) -> Result<Json<WFD>, AppError> {
    s.wfd.fetch(wfd_id, version)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::NOT_FOUND))
}
```

- [ ] **Step 2: Implement wfe routes**

Actor is extracted from three headers: `X-Actor-Orgu`, `X-Actor-User`, `X-Actor-Role`.

```rust
// crates/server/src/routes/wfe.rs
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::actor::Actor;
use crate::{error::AppError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",                       post(start_wfe).get(list_wfe))
        .route("/:id/actions",            post(apply_action))
        .route("/:id",                    get(query_wfe))
        .route("/:id/possible-actions",   get(possible_actions))
        .with_state(state)
}

fn extract_actor(headers: &HeaderMap) -> Result<Actor, AppError> {
    let orgu_id = parse_uuid_header(headers, "x-actor-orgu")?;
    let user_id = parse_uuid_header(headers, "x-actor-user")?;
    let role = headers
        .get("x-actor-role")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError("X-Actor-Role header required".into(), StatusCode::BAD_REQUEST))?
        .to_string();
    Ok(Actor { orgu_id, user_id, role })
}

fn parse_uuid_header(headers: &HeaderMap, name: &str) -> Result<Uuid, AppError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError(
            format!("{name} header required (UUID)"),
            StatusCode::BAD_REQUEST,
        ))
}

#[derive(Deserialize)]
struct StartBody {
    wfd_id:  Uuid,
    version: u32,
    #[serde(default)]
    input:   Value,
}

async fn start_wfe(
    State(s): State<AppState>,
    headers:  HeaderMap,
    Json(body): Json<StartBody>,
) -> Result<Json<wf_wfe::executor::WfeStartResult>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .start(body.wfd_id, body.version, &actor, &body.input)
        .await
        .map(Json)
        .map_err(AppError::from)
}

#[derive(Deserialize)]
struct ApplyBody {
    action: String,
    #[serde(default)]
    input:  Value,
}

async fn apply_action(
    State(s): State<AppState>,
    headers:  HeaderMap,
    Path(wfe_id): Path<Uuid>,
    Json(body): Json<ApplyBody>,
) -> Result<Json<wf_wfe::executor::WfeApplyResult>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor
        .apply(wfe_id, &actor, &body.action, &body.input)
        .await
        .map(Json)
        .map_err(AppError::from)
}

async fn query_wfe(
    State(s): State<AppState>,
    headers:  HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<wf_wfe::executor::WfeView>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor.query(wfe_id, &actor).await.map(Json).map_err(AppError::from)
}

async fn possible_actions(
    State(s): State<AppState>,
    headers:  HeaderMap,
    Path(wfe_id): Path<Uuid>,
) -> Result<Json<Vec<String>>, AppError> {
    let actor = extract_actor(&headers)?;
    s.executor.possible_actions(wfe_id, &actor).await.map(Json).map_err(AppError::from)
}

async fn list_wfe(
    State(s): State<AppState>,
    headers:  HeaderMap,
) -> Result<Json<Vec<wf_wfe::models::WfeRow>>, AppError> {
    // Extract orgtnt_id from actor's org — simplified: use X-Actor-Orgu tenant
    let actor = extract_actor(&headers)?;
    wf_wfe::repo::wfe::list_by_tenant(&s.pool, actor.orgu_id)
        .await
        .map(Json)
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))
}
```

> **Note on list_wfe:** The list endpoint does a basic tenant filter. Full listable rule checking (from WFD `listable` field) is not implemented in this sprint — it's noted as a follow-up.

- [ ] **Step 3: Verify full workspace compile**

```bash
cargo build --workspace 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/server/src/routes/
git commit -m "feat: server wfd and wfe routes — full REST API wired up"
```

---

## Task 9: Smoke Test — Start Server + Verify Endpoints

- [ ] **Step 1: Copy `.env.example` to `.env` and fill in values**

```bash
cp .env.example .env
# Edit DATABASE_URL to point to your local postgres instance
```

- [ ] **Step 2: Apply migrations**

```bash
psql "$DATABASE_URL" -f migrations/org/20260521000001_initial.sql
psql "$DATABASE_URL" -f migrations/wf/20260521000001_initial.sql
```

- [ ] **Step 3: Start the server**

```bash
cargo run -p wf-server 2>&1 &
sleep 2
```

Expected: `listening on 0.0.0.0:3000`

- [ ] **Step 4: Verify org endpoint responds**

```bash
curl -s http://localhost:3000/org/orgtnt | jq .
```

Expected: `[]` (empty array — no data seeded yet, but endpoint responds 200).

- [ ] **Step 5: Verify WFD upload**

```bash
curl -s -X POST http://localhost:3000/wfd \
  -H 'Content-Type: application/json' \
  -d '{
    "orgtnt_id": "00000000-0000-0000-0000-000000000001",
    "id": "00000000-0000-0000-0000-000000000001",
    "name": "test-workflow",
    "version": 1,
    "context": {},
    "start": [{
      "c_a": [{"c_orgu": "self", "c_r": [["self", "clerk"]]}],
      "wfes_effects": {"set": {"status": "pending"}},
      "wft": {"c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}]}
    }],
    "actions": {"approve": {"name": "approve", "input": {}}},
    "transitions": [{
      "id": "t1", "when": "$status == '\''pending'\''", "action": "approve",
      "c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}],
      "wfes_effects": {"set": {"status": "approved"}},
      "wft": {"c_a": []}
    }],
    "listable": [],
    "terminal_when": "$status == '\''approved'\''"
  }' | jq .
```

Expected: `{"wfd_id": "...", "version": 1}`

- [ ] **Step 6: Kill server and commit**

```bash
pkill -f wf-server
git add -A
git commit -m "feat: Plan 2 complete — wfd/wfe/server crates running, full REST API"
```

---

## Self-Review Checklist

### Spec Coverage

| Spec section | Task(s) |
|---|---|
| wfd crate: storage.rs (OpenDAL local/S3) | Task 1 |
| wfd crate: wfd_meta repo (insert, get, list) | Task 1 |
| wfd crate: WfdAdapter implements WfdPort | Task 2 |
| wfd crate: upload stores JSON in OpenDAL | Task 2 |
| wfe crate: wf.wfe repo (create, get, update) | Task 3 |
| wfe crate: wf.wfe_dynctx insert-only | Task 3 |
| wfe crate: wf.wfah append-only | Task 3 |
| wfe OrgAdapter implements OrgPort | Task 4 |
| wfe WfeAdapter implements WfePort | Task 4 |
| WfeExecutor start/apply/query/possible_actions | Task 5 |
| WFES.current_c_a cached in DB | Task 5 |
| WFES.end_response on terminal | Task 5 |
| server config + state + error | Task 6 |
| server /org/* routes (orgtnt, orgt, orgu, traverse) | Task 7 |
| server /wfd/* routes (upload, list, get) | Task 8 |
| server /wfe/* routes (start, apply, query, list, possible-actions) | Task 8 |
| Actor via X-Actor-Orgu/User/Role headers | Task 8 |
| Smoke test | Task 9 |

### Out of Scope (Confirmed)

- Autoexec node execution (rest/sql/calc) — types defined in wfe-core, execution not wired
- TRIGGER firing — type defined, execution not wired
- Full listable rule checking in GET /wfe — simplified tenant filter only
- JWT authentication — structural actor headers only
