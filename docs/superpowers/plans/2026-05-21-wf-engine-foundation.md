# WF Engine — Foundation Implementation Plan (Plan 1/2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the workspace skeleton, self-contained `org` crate (org domain + ORGTRVLANG), and `wfe-core` crate (pure engine: types, ports, state machine, ZEN evaluation) — no HTTP, no running DB required for wfe-core tests.

**Architecture:** Cargo workspace with 5 crates. `org` crate is fully self-contained (no internal deps); migrates existing `org-api` traversal code and adds `user_role` repo. `wfe-core` is pure Rust (no I/O): defines domain types, port traits, and the state machine. Port traits use `async-trait`; engine functions call `&dyn OrgPort` etc. only through those traits.

**Tech Stack:** Rust stable, sqlx 0.7 (postgres + ltree), serde/serde_json, async-trait, zen-engine 2, thiserror, uuid, chrono, tokio (tests only in wfe-core via tokio::test)

---

## File Map

```
workflow-engine/
├── Cargo.toml                                         CREATE
├── .env.example                                       CREATE
├── storage/wfd/.gitkeep                               CREATE
├── migrations/
│   ├── org/20260521000001_initial.sql                 CREATE
│   └── wf/20260521000001_initial.sql                  CREATE
└── crates/
    ├── org/
    │   ├── Cargo.toml                                 CREATE
    │   └── src/
    │       ├── lib.rs                                 CREATE
    │       ├── error.rs                               CREATE
    │       ├── models.rs                              CREATE
    │       ├── repo/
    │       │   ├── mod.rs                             CREATE
    │       │   ├── orgtnt.rs                          CREATE
    │       │   ├── orgt.rs                            CREATE
    │       │   ├── orgu.rs                            CREATE
    │       │   └── user_role.rs                       CREATE
    │       └── traversal/
    │           ├── mod.rs                             CREATE
    │           ├── parser.rs                          MIGRATE from org-api/src/traversal/pipeline.rs
    │           └── executor.rs                        MIGRATE from org-api/src/traversal/executor.rs
    └── wfe-core/
        ├── Cargo.toml                                 CREATE
        └── src/
            ├── lib.rs                                 CREATE
            ├── error.rs                               CREATE
            ├── types/
            │   ├── mod.rs                             CREATE
            │   ├── actor.rs                           CREATE
            │   ├── wfd.rs                             CREATE
            │   ├── wfe.rs                             CREATE
            │   ├── dynctx.rs                          CREATE
            │   └── wfah.rs                            CREATE
            ├── ports.rs                               CREATE
            ├── zen.rs                                 CREATE
            └── engine/
                ├── mod.rs                             CREATE
                ├── dynctx_apply.rs                    CREATE
                ├── c_a_resolver.rs                    CREATE
                ├── permission.rs                      CREATE
                ├── visibility.rs                      CREATE
                └── transition.rs                      CREATE
```

---

## Task 1: Workspace Cargo.toml + Crate Skeletons

**Files:**
- Create: `Cargo.toml`
- Create: `crates/org/Cargo.toml`
- Create: `crates/wfe-core/Cargo.toml`
- Create: `.env.example`
- Create: `storage/wfd/.gitkeep`

- [ ] **Step 1: Create workspace root `Cargo.toml`**

```toml
# workflow-engine/Cargo.toml
[workspace]
members  = ["crates/org", "crates/wfe-core", "crates/wfd", "crates/wfe", "crates/server"]
resolver = "2"

[workspace.dependencies]
axum               = "0.7"
tokio              = { version = "1", features = ["full"] }
sqlx               = { version = "0.7", features = ["postgres", "runtime-tokio-rustls", "uuid", "chrono", "json"] }
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
thiserror          = "1"
uuid               = { version = "1", features = ["v4", "serde"] }
chrono             = { version = "0.4", features = ["serde"] }
async-trait        = "0.1"
opendal            = { version = "0.50", features = ["services-fs", "services-s3"] }
zen-engine         = "2"
tracing            = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
tower-http         = { version = "0.5", features = ["cors"] }
dotenvy            = "0.15"
```

- [ ] **Step 2: Create stub directories and Cargo.toml for `org`**

```bash
mkdir -p crates/org/src/repo crates/org/src/traversal
```

```toml
# crates/org/Cargo.toml
[package]
name    = "wf-org"
version = "0.1.0"
edition = "2021"

[dependencies]
sqlx       = { workspace = true }
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
uuid       = { workspace = true }
chrono     = { workspace = true }
tracing    = { workspace = true }
```

- [ ] **Step 3: Create stub directories and Cargo.toml for `wfe-core`**

```bash
mkdir -p crates/wfe-core/src/types crates/wfe-core/src/engine
```

```toml
# crates/wfe-core/Cargo.toml
[package]
name    = "wfe-core"
version = "0.1.0"
edition = "2021"

[dependencies]
serde      = { workspace = true }
serde_json = { workspace = true }
thiserror  = { workspace = true }
uuid       = { workspace = true }
chrono     = { workspace = true }
async-trait = { workspace = true }
zen-engine = { workspace = true }
tracing    = { workspace = true }

[dev-dependencies]
tokio = { workspace = true }
```

- [ ] **Step 4: Create `.env.example` and storage placeholder**

```bash
# .env.example
DATABASE_URL=postgres://postgres:postgres@localhost:5432/wf_engine
PORT=3000
STORAGE_BACKEND=local
STORAGE_PATH=./storage
RUST_LOG=info
```

```bash
mkdir -p storage/wfd
touch storage/wfd/.gitkeep
```

- [ ] **Step 5: Verify workspace compiles**

```bash
cargo check --workspace 2>&1 | head -30
```

Expected: errors about missing `lib.rs` files — that is correct at this stage. No "could not find Cargo.toml" errors.

- [ ] **Step 6: Create empty `lib.rs` stubs for both crates so workspace compiles**

```bash
echo 'pub mod error;' > crates/org/src/lib.rs
touch crates/org/src/error.rs
echo '' > crates/wfe-core/src/lib.rs
```

```bash
cargo check --workspace 2>&1 | grep -E "^error"
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/ .env.example storage/
git commit -m "feat: workspace skeleton with org and wfe-core crate stubs"
```

---

## Task 2: DB Migrations

**Files:**
- Create: `migrations/org/20260521000001_initial.sql`
- Create: `migrations/wf/20260521000001_initial.sql`

- [ ] **Step 1: Create org schema migration**

```bash
mkdir -p migrations/org migrations/wf
```

```sql
-- migrations/org/20260521000001_initial.sql
CREATE EXTENSION IF NOT EXISTS ltree;
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE SCHEMA IF NOT EXISTS org;

CREATE TABLE org.orgtnt (
    orgtnt_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    name       text        NOT NULL,
    code       text        NOT NULL UNIQUE,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE org.orgt (
    orgt_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name        text        NOT NULL,
    description text,
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    updated_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX orgt_orgtnt_idx ON org.orgt(orgtnt_id);

CREATE TABLE org.orgu (
    orgu_id    uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgu_type  jsonb       NOT NULL,
    name       text        NOT NULL,
    metadata   jsonb,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX orgu_type_gin     ON org.orgu USING gin(orgu_type);
CREATE INDEX orgu_active_btree ON org.orgu(is_active);
CREATE UNIQUE INDEX orgu_seed_code_unique
    ON org.orgu ((metadata->>'code'))
    WHERE metadata ? 'code';

CREATE TABLE org.orgt_orgu (
    orgt_id        uuid  NOT NULL REFERENCES org.orgt(orgt_id),
    orgu_id        uuid  NOT NULL REFERENCES org.orgu(orgu_id),
    orgtnt_id      uuid  NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    parent_orgu_id uuid  REFERENCES org.orgu(orgu_id),
    path           ltree NOT NULL,
    is_active      boolean     NOT NULL DEFAULT true,
    created_at     timestamptz NOT NULL DEFAULT now(),
    updated_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (orgt_id, orgu_id),
    UNIQUE (orgt_id, path)
);
CREATE INDEX orgt_orgu_path_gist    ON org.orgt_orgu USING gist(path);
CREATE INDEX orgt_orgu_orgt_btree   ON org.orgt_orgu(orgt_id);
CREATE INDEX orgt_orgu_orgtnt_btree ON org.orgt_orgu(orgtnt_id);
CREATE INDEX orgt_orgu_parent_btree ON org.orgt_orgu(parent_orgu_id);
CREATE INDEX orgt_orgu_active_btree ON org.orgt_orgu(is_active);

CREATE TABLE org.u (
    u_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    username   text        NOT NULL,
    full_name  text        NOT NULL,
    email      text,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, username)
);
CREATE INDEX u_orgtnt_idx ON org.u(orgtnt_id);

CREATE TABLE org.u_orgu (
    u_orgu_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id       uuid        NOT NULL REFERENCES org.u(u_id),
    orgu_id    uuid        NOT NULL REFERENCES org.orgu(orgu_id),
    is_primary boolean     NOT NULL DEFAULT false,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (u_id, orgu_id)
);
CREATE INDEX u_orgu_tenant_u_idx ON org.u_orgu(orgtnt_id, u_id);
CREATE INDEX u_orgu_u_idx        ON org.u_orgu(u_id);
CREATE INDEX u_orgu_orgu_idx     ON org.u_orgu(orgu_id);

CREATE TABLE org.r (
    r_id         uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    name         text        NOT NULL,
    display_name text        NOT NULL,
    is_active    boolean     NOT NULL DEFAULT true,
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name)
);
CREATE INDEX r_orgtnt_idx ON org.r(orgtnt_id);

CREATE TABLE org.ur (
    ur_id      uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL REFERENCES org.orgtnt(orgtnt_id),
    u_id       uuid        NOT NULL REFERENCES org.u(u_id),
    r_id       uuid        NOT NULL REFERENCES org.r(r_id),
    orgu_id    uuid        REFERENCES org.orgu(orgu_id),
    orgu_scope text,
    ur_type    text        NOT NULL DEFAULT 'granted'
               CHECK (ur_type IN ('inherited','granted','excluded')),
    valid_from  timestamptz,
    valid_until timestamptz,
    created_at  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX ur_tenant_u_idx ON org.ur(orgtnt_id, u_id);
CREATE INDEX ur_u_idx        ON org.ur(u_id);
CREATE INDEX ur_r_idx        ON org.ur(r_id);
CREATE INDEX ur_orgu_idx     ON org.ur(orgu_id);
```

- [ ] **Step 2: Create wf schema migration**

```sql
-- migrations/wf/20260521000001_initial.sql
CREATE SCHEMA IF NOT EXISTS wf;

CREATE TABLE wf.wfd_meta (
    wfd_id     uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id  uuid        NOT NULL,
    name       text        NOT NULL,
    version    integer     NOT NULL DEFAULT 1,
    s3_key     text        NOT NULL,
    is_active  boolean     NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name, version)
);
CREATE INDEX wfd_meta_orgtnt_idx ON wf.wfd_meta(orgtnt_id);

CREATE TABLE wf.wfe (
    wfe_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    wfd_id       uuid        NOT NULL,
    wfd_version  integer     NOT NULL,
    status       text        NOT NULL CHECK (status IN ('active','terminal','error')),
    current_c_a  jsonb       NOT NULL DEFAULT '[]',
    end_response jsonb,
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX wfe_orgtnt_idx ON wf.wfe(orgtnt_id);
CREATE INDEX wfe_status_idx ON wf.wfe(status);
CREATE INDEX wfe_wfd_idx    ON wf.wfe(wfd_id);

CREATE TABLE wf.wfe_dynctx (
    dynctx_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    ctx        jsonb       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);
CREATE INDEX wfe_dynctx_wfe_idx ON wf.wfe_dynctx(wfe_id);

CREATE TABLE wf.wfah (
    wfah_id    uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    action     text        NOT NULL,
    actor      jsonb       NOT NULL,
    input      jsonb,
    applied_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);
CREATE INDEX wfah_wfe_idx ON wf.wfah(wfe_id);
```

- [ ] **Step 3: Apply migrations to local DB**

```bash
psql "$DATABASE_URL" -f migrations/org/20260521000001_initial.sql
psql "$DATABASE_URL" -f migrations/wf/20260521000001_initial.sql
```

Expected: no errors. Tables visible in `\dt org.*` and `\dt wf.*`.

- [ ] **Step 4: Commit**

```bash
git add migrations/
git commit -m "feat: add org and wf schema migrations"
```

---

## Task 3: `org` Crate — Models + Error

**Files:**
- Create: `crates/org/src/error.rs`
- Create: `crates/org/src/models.rs`
- Modify: `crates/org/src/lib.rs`

- [ ] **Step 1: Write error type**

```rust
// crates/org/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OrgError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
```

- [ ] **Step 2: Write models**

```rust
// crates/org/src/models.rs
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct Orgtnt {
    pub orgtnt_id:  Uuid,
    pub name:       String,
    pub code:       String,
    pub is_active:  bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgt {
    pub orgt_id:     Uuid,
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub description: Option<String>,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgu {
    pub orgu_id:        Uuid,
    pub orgt_id:        Uuid,
    pub orgtnt_id:      Uuid,
    pub parent_orgu_id: Option<Uuid>,
    pub path:           String,
    pub orgu_type:      serde_json::Value,
    pub name:           String,
    pub metadata:       Option<serde_json::Value>,
    pub is_active:      bool,
    pub created_at:     DateTime<Utc>,
    pub updated_at:     DateTime<Utc>,
}

/// Minimal org unit view used by wfe-core via OrgPort.
#[derive(Debug, Clone, Serialize)]
pub struct OrgUnit {
    pub orgu_id:   Uuid,
    pub orgu_type: serde_json::Value,
    pub path:      String,
}

impl From<Orgu> for OrgUnit {
    fn from(o: Orgu) -> Self {
        Self { orgu_id: o.orgu_id, orgu_type: o.orgu_type, path: o.path }
    }
}
```

- [ ] **Step 3: Update `lib.rs`**

```rust
// crates/org/src/lib.rs
pub mod error;
pub mod models;
pub mod repo;
pub mod traversal;
```

- [ ] **Step 4: Create stub `repo/mod.rs`**

```rust
// crates/org/src/repo/mod.rs
pub mod orgtnt;
pub mod orgt;
pub mod orgu;
pub mod user_role;
```

- [ ] **Step 5: Create stub traversal `mod.rs`**

```rust
// crates/org/src/traversal/mod.rs
pub mod parser;
pub mod executor;
```

- [ ] **Step 6: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: errors about missing files in repo/ and traversal/ — correct. No type errors.

---

## Task 4: `org` Crate — Traversal (Migrate from `org-api`)

**Files:**
- Create: `crates/org/src/traversal/parser.rs` (from `org-api/src/traversal/pipeline.rs`)
- Create: `crates/org/src/traversal/executor.rs` (from `org-api/src/traversal/executor.rs`)

- [ ] **Step 1: Copy parser — rename `pipeline.rs` → `parser.rs`, keep content identical**

```bash
cp org-api/src/traversal/pipeline.rs crates/org/src/traversal/parser.rs
```

The `parser.rs` content is identical to `pipeline.rs`. The public types exported are: `Pipeline`, `Step`, `FilterExpr`, `TypeFilter`, `ParseError`, and `pub fn parse(expr: &str) -> Result<Pipeline, ParseError>`.

- [ ] **Step 2: Copy executor — update schema prefix in all SQL queries**

```bash
cp org-api/src/traversal/executor.rs crates/org/src/traversal/executor.rs
```

Then update the two constants at the top of `executor.rs`. Replace:

```rust
// OLD (org-api version — public schema, no prefix)
const MEMBERS: &str =
    "WITH members AS ( \
         SELECT o.orgu_id, oo.orgtnt_id, oo.orgt_id, oo.parent_orgu_id, \
                oo.path, o.orgu_type, o.name, o.metadata, \
                (o.is_active AND oo.is_active) AS is_active, \
                o.created_at, o.updated_at \
         FROM orgu o \
         JOIN orgt_orgu oo ON o.orgu_id = oo.orgu_id \
         WHERE oo.orgt_id = $2 AND o.is_active = true AND oo.is_active = true \
     )";
```

With:

```rust
// NEW (org schema prefix)
const MEMBERS: &str =
    "WITH members AS ( \
         SELECT o.orgu_id, oo.orgtnt_id, oo.orgt_id, oo.parent_orgu_id, \
                oo.path, o.orgu_type, o.name, o.metadata, \
                (o.is_active AND oo.is_active) AS is_active, \
                o.created_at, o.updated_at \
         FROM org.orgu o \
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id \
         WHERE oo.orgt_id = $2 AND o.is_active = true AND oo.is_active = true \
     )";
```

- [ ] **Step 3: Update imports in executor.rs — change `crate::models::Orgu` to `crate::models::Orgu`**

The executor uses `crate::error::AppError` in org-api. Change to `crate::error::OrgError`:

In `executor.rs`, replace all occurrences of:
- `AppError` → `OrgError`
- `AppError::Database` → `OrgError::Database`
- `use crate::{error::AppError, models::Orgu};` → `use crate::{error::OrgError, models::Orgu};`

Also update the `execute` and related function signatures to return `Result<..., OrgError>`.

- [ ] **Step 4: Run existing parser unit tests**

```bash
cargo test -p wf-org traversal::parser 2>&1
```

Expected: all tests pass (the parser logic is unchanged from org-api).

- [ ] **Step 5: Commit**

```bash
git add crates/org/src/traversal/
git commit -m "feat: migrate ORGTRVLANG parser and executor into org crate"
```

---

## Task 5: `org` Crate — Repo Modules

**Files:**
- Create: `crates/org/src/repo/orgtnt.rs`
- Create: `crates/org/src/repo/orgt.rs`
- Create: `crates/org/src/repo/orgu.rs`
- Create: `crates/org/src/repo/user_role.rs`

- [ ] **Step 1: Write `orgtnt.rs`**

```rust
// crates/org/src/repo/orgtnt.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgtnt};

pub async fn list(pool: &PgPool) -> Result<Vec<Orgtnt>, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "SELECT orgtnt_id, name, code, is_active, created_at, updated_at
         FROM org.orgtnt ORDER BY name"
    )
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Orgtnt, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "SELECT orgtnt_id, name, code, is_active, created_at, updated_at
         FROM org.orgtnt WHERE orgtnt_id = $1"
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}
```

- [ ] **Step 2: Write `orgt.rs`**

```rust
// crates/org/src/repo/orgt.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgt};

pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Orgt>, OrgError> {
    sqlx::query_as::<_, Orgt>(
        "SELECT orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at
         FROM org.orgt WHERE orgtnt_id = $1 ORDER BY name"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}
```

- [ ] **Step 3: Write `orgu.rs`**

```rust
// crates/org/src/repo/orgu.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::Orgu};

const SEL: &str =
    "o.orgu_id, oo.orgt_id, oo.orgtnt_id, oo.parent_orgu_id,
     oo.path::text AS path, o.orgu_type, o.name, o.metadata,
     (o.is_active AND oo.is_active) AS is_active,
     o.created_at, o.updated_at";

pub async fn list_by_tree(pool: &PgPool, orgt_id: Uuid) -> Result<Vec<Orgu>, OrgError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "SELECT {SEL}
         FROM org.orgu o
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
         WHERE oo.orgt_id = $1 AND o.is_active = true AND oo.is_active = true
         ORDER BY oo.path"
    ))
    .bind(orgt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, orgu_id: Uuid) -> Result<Orgu, OrgError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "SELECT {SEL}
         FROM org.orgu o
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
         WHERE o.orgu_id = $1 AND o.is_active = true AND oo.is_active = true
         LIMIT 1"
    ))
    .bind(orgu_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))
}

/// Returns the orgt_id for an orgu — needed by the traversal executor.
pub async fn get_orgt_id(pool: &PgPool, orgu_id: Uuid) -> Result<Uuid, OrgError> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT orgt_id FROM org.orgt_orgu WHERE orgu_id = $1 LIMIT 1"
    )
    .bind(orgu_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgu {orgu_id}")))
}
```

- [ ] **Step 4: Write `user_role.rs`**

```rust
// crates/org/src/repo/user_role.rs
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::OrgUnit, traversal::{executor, parser}};

/// Returns true if user holds the given role in the given orgu,
/// respecting timeslice validity and excluding 'excluded' assignments.
pub async fn check_user_role(
    pool:      &PgPool,
    user_id:   Uuid,
    orgu_id:   Uuid,
    role_name: &str,
) -> Result<bool, OrgError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (
             SELECT 1 FROM org.ur u
             JOIN org.r r ON u.r_id = r.r_id
             WHERE u.u_id    = $1
               AND u.orgu_id = $2
               AND r.name    = $3
               AND u.ur_type != 'excluded'
               AND (u.valid_from  IS NULL OR u.valid_from  <= now())
               AND (u.valid_until IS NULL OR u.valid_until >  now())
         )"
    )
    .bind(user_id)
    .bind(orgu_id)
    .bind(role_name)
    .fetch_one(pool)
    .await?;
    Ok(exists)
}

/// Resolves an ORGTRVLANG expression from an anchor ORGU and returns OrgUnit results.
/// For absolute expressions starting with "*:" (e.g. "*:[type:branch]"), anchor_orgu_id
/// is used only to determine the orgtnt scope.
pub async fn resolve_orgu(
    pool:           &PgPool,
    anchor_orgu_id: Uuid,
    expr:           &str,
    orgtnt_id:      Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
    if let Some(type_expr) = expr.strip_prefix("*:") {
        return resolve_global_type(pool, type_expr, orgtnt_id).await;
    }

    let orgt_id = super::orgu::get_orgt_id(pool, anchor_orgu_id).await?;
    let pipeline = parser::parse(expr)
        .map_err(|e| OrgError::BadRequest(e.to_string()))?;
    let orgus = executor::execute(pool, anchor_orgu_id, orgt_id, &pipeline).await?;
    Ok(orgus.into_iter().map(OrgUnit::from).collect())
}

/// Handles "*:[type:branch]" — all orgus of a given type within the tenant.
async fn resolve_global_type(
    pool:      &PgPool,
    type_expr: &str,    // "[type:branch]" or "[key:val]"
    orgtnt_id: Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
    // Parse "[type:branch]" → key="type", val="branch"
    let inner = type_expr
        .trim_start_matches('[')
        .trim_end_matches(']');
    let (key, val) = inner
        .split_once(':')
        .ok_or_else(|| OrgError::BadRequest(format!("invalid type expr: {type_expr}")))?;

    let rows = sqlx::query_as::<_, (Uuid, serde_json::Value, String)>(
        "SELECT o.orgu_id, o.orgu_type, oo.path::text
         FROM org.orgu o
         JOIN org.orgt_orgu oo ON o.orgu_id = oo.orgu_id
         WHERE oo.orgtnt_id  = $1
           AND o.is_active   = true
           AND oo.is_active  = true
           AND (o.orgu_type ? '*'
                OR o.orgu_type->>$2 = $3
                OR o.orgu_type->$2 @> to_jsonb($3::text))"
    )
    .bind(orgtnt_id)
    .bind(key)
    .bind(val)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(orgu_id, orgu_type, path)| OrgUnit { orgu_id, orgu_type, path })
        .collect())
}
```

- [ ] **Step 5: Verify `org` crate compiles**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/org/
git commit -m "feat: org crate — models, repos (orgtnt/orgt/orgu/user_role), traversal migrated"
```

---

## Task 6: `wfe-core` — Domain Types

**Files:**
- Create: `crates/wfe-core/src/types/actor.rs`
- Create: `crates/wfe-core/src/types/wfe.rs`
- Create: `crates/wfe-core/src/types/dynctx.rs`
- Create: `crates/wfe-core/src/types/wfah.rs`
- Create: `crates/wfe-core/src/types/mod.rs`

- [ ] **Step 1: Write `actor.rs`**

```rust
// crates/wfe-core/src/types/actor.rs
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Exact (ORGU, (U, R)) triple — the only valid actor representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub orgu_id: Uuid,
    pub user_id: Uuid,
    pub role:    String,
}

/// Minimal org unit returned by OrgPort.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgUnit {
    pub orgu_id:   Uuid,
    pub orgu_type: serde_json::Value,
    pub path:      String,
}

/// A resolved (orgu, role) pair — one entry in the candidate actor set.
/// Any user holding this role in this orgu satisfies the candidate requirement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateActor {
    pub orgu_id: Uuid,
    pub role:    String,
}

/// One rule in a c_a array (OR across rules, AND within a rule).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaRule {
    pub c_orgu: COrguExpr,
    #[serde(default)]
    pub c_r:    Vec<[String; 2]>,   // [orgu_scope, role_name]
    #[serde(default)]
    pub c_u:    Vec<String>,
}

/// Two forms of c_orgu as defined in CLAUDE.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum COrguExpr {
    /// {"from": "$ctx.field.orgu", "traverse": "self"}
    Anchored { from: String, traverse: String },
    /// ORGTRVLANG expr string or "*:[type:branch]"
    Expr(String),
}
```

- [ ] **Step 2: Write `wfe.rs`**

```rust
// crates/wfe-core/src/types/wfe.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WfeStatus {
    Active,
    Terminal,
    Error,
}
```

- [ ] **Step 3: Write `dynctx.rs`**

```rust
// crates/wfe-core/src/types/dynctx.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Immutable DynCtx snapshot. apply_effects always returns a new instance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DynCtx(pub Value);

impl DynCtx {
    pub fn empty() -> Self {
        Self(Value::Object(serde_json::Map::new()))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Merges a flat map of key→value into a new DynCtx. Never mutates self.
    pub fn merge(&self, patch: serde_json::Map<String, Value>) -> Self {
        let mut map = match &self.0 {
            Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        for (k, v) in patch {
            map.insert(k, v);
        }
        Self(Value::Object(map))
    }
}
```

- [ ] **Step 4: Write `wfah.rs`**

```rust
// crates/wfe-core/src/types/wfah.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use super::actor::Actor;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfahEntry {
    pub seq:        u32,
    pub action:     String,
    pub actor:      Actor,
    pub input:      Option<Value>,
    pub applied_at: DateTime<Utc>,
}

/// Append-only action history. push() returns a new Wfah — never mutates.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Wfah(pub Vec<WfahEntry>);

impl Wfah {
    pub fn empty() -> Self {
        Self(vec![])
    }

    /// Returns a new Wfah with the entry appended. seq = last_seq + 1.
    pub fn push(&self, action: String, actor: Actor, input: Option<Value>) -> Self {
        let seq = self.0.last().map(|e| e.seq + 1).unwrap_or(1);
        let mut entries = self.0.clone();
        entries.push(WfahEntry { seq, action, actor, input, applied_at: Utc::now() });
        Self(entries)
    }

    pub fn entries(&self) -> &[WfahEntry] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() }
    }

    #[test]
    fn push_increments_seq() {
        let wfah = Wfah::empty();
        let w1 = wfah.push("start".into(), actor(), None);
        let w2 = w1.push("approve".into(), actor(), None);
        assert_eq!(w1.entries()[0].seq, 1);
        assert_eq!(w2.entries()[1].seq, 2);
    }

    #[test]
    fn push_does_not_mutate_original() {
        let wfah = Wfah::empty();
        let _w1 = wfah.push("start".into(), actor(), None);
        assert_eq!(wfah.entries().len(), 0); // original unchanged
    }
}
```

- [ ] **Step 5: Write `types/mod.rs`**

```rust
// crates/wfe-core/src/types/mod.rs
pub mod actor;
pub mod dynctx;
pub mod wfah;
pub mod wfd;
pub mod wfe;

pub use actor::{Actor, CaRule, COrguExpr, CandidateActor, OrgUnit};
pub use dynctx::DynCtx;
pub use wfah::{Wfah, WfahEntry};
pub use wfd::{WFD, Transition, StartRule, WftRule, WftCondition, WfesEffects, EffectValue};
pub use wfe::WfeStatus;
```

- [ ] **Step 6: Run Wfah tests**

```bash
cargo test -p wfe-core types::wfah 2>&1
```

Expected: 2 tests pass.

---

## Task 7: `wfe-core` — WFD Types

**Files:**
- Create: `crates/wfe-core/src/types/wfd.rs`

- [ ] **Step 1: Write WFD domain types**

```rust
// crates/wfe-core/src/types/wfd.rs
use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use super::actor::CaRule;

/// Top-level WFD document — mirrors the JSON structure in CLAUDE.md exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WFD {
    pub id:            Uuid,
    pub name:          String,
    pub version:       u32,
    pub description:   Option<String>,
    /// JSON Schema 2020-12 with x-visibility and x-wf-readonly extensions
    pub context:       Value,
    pub start:         Vec<StartRule>,
    pub actions:       HashMap<String, ActionDef>,
    pub transitions:   Vec<Transition>,
    pub listable:      Vec<ListableRule>,
    pub terminal_when: String,   // ZEN expression
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDef {
    pub name:        String,
    pub description: Option<String>,
    #[serde(default)]
    pub input: ActionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActionInput {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRule {
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    pub wft:          WftRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub id:           String,
    pub when:         String,   // ZEN expression evaluated against current WFES
    pub action:       String,   // key in WFD.actions map
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger:      Option<AutoexecDef>,
    pub wft:          WftRule,
}

/// wft has two forms: simple (c_a array) or conditional (branching on ZEN).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WftRule {
    Simple { c_a: Vec<CaRule> },
    Conditional { conditions: Vec<WftCondition> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WftCondition {
    pub when:                        String,   // ZEN expression
    #[serde(default)]
    pub terminal:                    bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wfe_end_response:            Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c_a:                         Option<Vec<CaRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger:                     Option<AutoexecDef>,
}

/// wfes_effects — {"set": {"field": value}} structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WfesEffects {
    #[serde(default)]
    pub set: HashMap<String, EffectValue>,
}

/// Values in wfes_effects.set — special strings ($actor etc.) or JSON refs or literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectValue {
    /// {"ref": "$ctx.field.path"} — dynamic reference into current DynCtx
    Ref { #[serde(rename = "ref")] path: String },
    /// Any JSON literal, including special strings "$actor", "$timestamp", "$wfe_id",
    /// "$action.input.field_name"
    Literal(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListableRule {
    pub c_a:  Vec<CaRule>,
    /// Optional ZEN condition — if present, rule only applies when condition is true
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Autoexec node definition — execution deferred to Plan 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoexecDef {
    #[serde(rename = "type")]
    pub kind:   String,   // "rest" | "sql" | "calc"
    #[serde(flatten)]
    pub params: Value,
}
```

- [ ] **Step 2: Write a unit test that round-trips a minimal WFD through serde**

```rust
// Add at the bottom of wfd.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wfd_roundtrip() {
        let json = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "test-wfd",
            "version": 1,
            "context": {},
            "start": [{
                "c_a": [{"c_orgu": "self", "c_r": [["self", "clerk"]]}],
                "wfes_effects": {"set": {"status": "pending"}},
                "wft": {"c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}]}
            }],
            "actions": {
                "approve": {"name": "approve", "input": {"required": [], "optional": []}}
            },
            "transitions": [{
                "id": "t1",
                "when": "$status == 'pending'",
                "action": "approve",
                "c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}],
                "wfes_effects": {"set": {"status": "approved"}},
                "wft": {"c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}]}
            }],
            "listable": [],
            "terminal_when": "$status == 'approved'"
        });

        let wfd: WFD = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(wfd.name, "test-wfd");
        assert_eq!(wfd.transitions.len(), 1);
        assert_eq!(wfd.transitions[0].id, "t1");

        // Confirm WftRule::Simple deserialized
        assert!(matches!(wfd.transitions[0].wft, WftRule::Simple { .. }));

        // Re-serialize and re-parse
        let back: WFD = serde_json::from_str(&serde_json::to_string(&wfd).unwrap()).unwrap();
        assert_eq!(back.name, wfd.name);
    }

    #[test]
    fn wft_conditional_deserializes() {
        let json = serde_json::json!({
            "conditions": [
                {"when": "$amount < 1000", "terminal": true,
                 "wfe_end_response": {"status": "approved"}},
                {"when": "$amount >= 1000", "c_a": [{"c_orgu": "self", "c_r": [["self", "director"]]}]}
            ]
        });
        let wft: WftRule = serde_json::from_value(json).unwrap();
        assert!(matches!(wft, WftRule::Conditional { .. }));
    }
}
```

- [ ] **Step 3: Run tests**

```bash
cargo test -p wfe-core types::wfd 2>&1
```

Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/wfe-core/src/types/
git commit -m "feat: wfe-core domain types — Actor, DynCtx, Wfah, WFD structures"
```

---

## Task 8: `wfe-core` — Error + Port Traits

**Files:**
- Create: `crates/wfe-core/src/error.rs`
- Create: `crates/wfe-core/src/ports.rs`
- Modify: `crates/wfe-core/src/lib.rs`

- [ ] **Step 1: Write `error.rs`**

```rust
// crates/wfe-core/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("permission denied: actor is not in candidate set for action '{0}'")]
    PermissionDenied(String),
    #[error("transition not found for action '{0}' in current state")]
    TransitionNotFound(String),
    #[error("wfe is terminal — no further actions accepted")]
    WfeTerminal,
    #[error("start rule not matched — actor not eligible to initiate this workflow")]
    StartNotEligible,
    #[error("zen evaluation error: {0}")]
    ZenEvaluation(String),
    #[error("invalid expression: {0}")]
    InvalidExpression(String),
    #[error("org port error: {0}")]
    OrgPort(String),
    #[error("wfd port error: {0}")]
    WfdPort(String),
    #[error("wfe port error: {0}")]
    WfePort(String),
    #[error("invalid wfd: {0}")]
    InvalidWfd(String),
    #[error("effect value error: {0}")]
    EffectValue(String),
}
```

- [ ] **Step 2: Write `ports.rs`**

```rust
// crates/wfe-core/src/ports.rs
use async_trait::async_trait;
use uuid::Uuid;
use crate::{
    error::EngineError,
    types::{actor::{OrgUnit, CandidateActor}, dynctx::DynCtx, wfah::WfahEntry, wfd::WFD, wfe::WfeStatus},
};
use serde_json::Value;

/// WFES — passed to WfePort and returned by WfePort::load_wfes.
/// Defined here to avoid circular imports.
#[derive(Debug, Clone)]
pub struct WFES {
    pub wfe_id:  Uuid,
    pub dynctx:  DynCtx,
    pub wfah:    crate::types::wfah::Wfah,
    pub status:  WfeStatus,
    pub orgtnt_id: Uuid,
    pub wfd_id:  Uuid,
    pub wfd_version: u32,
}

#[async_trait]
pub trait OrgPort: Send + Sync {
    /// Resolves an ORGTRVLANG expression from an anchor ORGU.
    /// For absolute "*:[type:X]" expressions, anchor_orgu_id may be any valid
    /// orgu in the tenant — only orgtnt_id is used for scope.
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr:           &str,
        orgtnt_id:      Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError>;

    async fn check_user_role(
        &self,
        user_id:   Uuid,
        orgu_id:   Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError>;
}

#[async_trait]
pub trait WfdPort: Send + Sync {
    async fn fetch(&self, wfd_id: Uuid, version: u32) -> Result<WFD, EngineError>;
}

#[async_trait]
pub trait WfePort: Send + Sync {
    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError>;

    /// Insert a new DynCtx snapshot (insert-only, never update).
    async fn persist_new_dynctx(
        &self,
        wfe_id: Uuid,
        ctx:    &DynCtx,
        seq:    u32,
    ) -> Result<(), EngineError>;

    async fn append_wfah(
        &self,
        wfe_id: Uuid,
        entry:  &WfahEntry,
    ) -> Result<(), EngineError>;

    async fn update_c_a(
        &self,
        wfe_id: Uuid,
        c_a:    &[CandidateActor],
    ) -> Result<(), EngineError>;

    async fn set_terminal(
        &self,
        wfe_id:       Uuid,
        end_response: &Value,
    ) -> Result<(), EngineError>;

    async fn create_wfe(
        &self,
        orgtnt_id:   Uuid,
        wfd_id:      Uuid,
        wfd_version: u32,
        initial_ctx: &DynCtx,
        initial_c_a: &[CandidateActor],
    ) -> Result<Uuid, EngineError>;   // returns new wfe_id
}
```

- [ ] **Step 3: Update `lib.rs`**

```rust
// crates/wfe-core/src/lib.rs
pub mod error;
pub mod engine;
pub mod ports;
pub mod types;
pub mod zen;

pub use error::EngineError;
pub use ports::{OrgPort, WfdPort, WfePort, WFES};
pub use types::*;
```

- [ ] **Step 4: Create `engine/mod.rs` stub**

```rust
// crates/wfe-core/src/engine/mod.rs
pub mod c_a_resolver;
pub mod dynctx_apply;
pub mod permission;
pub mod transition;
pub mod visibility;
```

- [ ] **Step 5: Create `zen.rs` stub**

```rust
// crates/wfe-core/src/zen.rs
use crate::{error::EngineError, ports::WFES};

pub fn evaluate(expr: &str, wfes: &WFES) -> Result<bool, EngineError> {
    let context = build_context(wfes);
    eval_expression(expr, &context)
}

fn build_context(wfes: &WFES) -> serde_json::Value {
    let mut map = serde_json::Map::new();

    // Expose all DynCtx top-level fields as $field
    if let serde_json::Value::Object(ctx_map) = wfes.dynctx.as_value() {
        for (k, v) in ctx_map {
            map.insert(format!("${k}"), v.clone());
        }
    }

    // Expose full WFAH as $wfah array
    let wfah_arr: serde_json::Value = wfes.wfah.entries()
        .iter()
        .map(|e| serde_json::json!({
            "action":     e.action,
            "actor":      e.actor,
            "applied_at": e.applied_at.to_rfc3339(),
        }))
        .collect();
    map.insert("$wfah".into(), wfah_arr);

    serde_json::Value::Object(map)
}

fn eval_expression(expr: &str, context: &serde_json::Value) -> Result<bool, EngineError> {
    // zen-engine 2.x expression evaluation
    // The zen_engine crate provides ZenEngine for evaluating expressions.
    // Note: verify the exact API against zen-engine 2.x docs.
    use zen_engine::ZenEngine;
    let engine = ZenEngine::default();
    let result = engine
        .evaluate_expression(expr, context)
        .map_err(|e| EngineError::ZenEvaluation(e.to_string()))?;

    result
        .result
        .as_bool()
        .ok_or_else(|| EngineError::ZenEvaluation(
            format!("expression '{expr}' did not evaluate to boolean")
        ))
}
```

> **Note:** Verify `zen_engine::ZenEngine::evaluate_expression` API against installed zen-engine 2.x. If the API differs, update `eval_expression` accordingly. The wrapper signature `evaluate(expr, wfes) → Result<bool>` must not change.

- [ ] **Step 6: Verify compile**

```bash
cargo check -p wfe-core 2>&1 | grep "^error"
```

Expected: no errors (stubs for engine modules not yet written will produce "unresolved module" errors — create empty files to fix):

```bash
for f in c_a_resolver dynctx_apply permission transition visibility; do
  touch crates/wfe-core/src/engine/${f}.rs
done
cargo check -p wfe-core 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/wfe-core/src/
git commit -m "feat: wfe-core error types, port traits, ZEN wrapper stub"
```

---

## Task 9: `wfe-core` — DynCtx Apply Effects

**Files:**
- Create: `crates/wfe-core/src/engine/dynctx_apply.rs`

- [ ] **Step 1: Write the unit test first (TDD)**

```rust
// crates/wfe-core/src/engine/dynctx_apply.rs
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use serde_json::json;
    use crate::types::{actor::Actor, dynctx::DynCtx, wfd::{WfesEffects, EffectValue}};
    use std::collections::HashMap;

    fn actor() -> Actor {
        Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() }
    }

    #[test]
    fn sets_literal_string() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert("status".into(), EffectValue::Literal(json!("pending")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert_eq!(new_ctx.get("status"), Some(&json!("pending")));
    }

    #[test]
    fn sets_actor_special() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert("initiated_by".into(), EffectValue::Literal(json!("$actor")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        let stored = new_ctx.get("initiated_by").unwrap();
        assert_eq!(stored["orgu_id"], json!(actor.orgu_id));
        assert_eq!(stored["role"], json!("clerk"));
    }

    #[test]
    fn sets_action_input_ref() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert("amount".into(), EffectValue::Literal(json!("$action.input.amount")));
        let input = json!({"amount": 500});

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "submit", &input).unwrap();
        assert_eq!(new_ctx.get("amount"), Some(&json!(500)));
    }

    #[test]
    fn original_ctx_unchanged() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert("status".into(), EffectValue::Literal(json!("pending")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert!(ctx.get("status").is_none());       // original untouched
        assert!(new_ctx.get("status").is_some());   // new has it
    }

    #[test]
    fn ctx_ref_reads_existing_ctx_field() {
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("applicant_orgu".into(), json!({"orgu": "abc-uuid"}));
            m
        });
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert("orgu_copy".into(), EffectValue::Ref { path: "$ctx.applicant_orgu.orgu".into() });

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert_eq!(new_ctx.get("orgu_copy"), Some(&json!("abc-uuid")));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p wfe-core engine::dynctx_apply 2>&1 | tail -5
```

Expected: FAIL — functions not yet defined.

- [ ] **Step 3: Implement `apply`**

```rust
// crates/wfe-core/src/engine/dynctx_apply.rs
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
use crate::{
    error::EngineError,
    types::{actor::Actor, dynctx::DynCtx, wfd::{EffectValue, WfesEffects}},
};

/// Applies wfes_effects to produce a new immutable DynCtx. Never mutates `ctx`.
pub fn apply(
    ctx:     &DynCtx,
    effects: &WfesEffects,
    actor:   &Actor,
    wfe_id:  Uuid,
    action:  &str,
    input:   &Value,
) -> Result<DynCtx, EngineError> {
    let mut patch = serde_json::Map::new();
    for (field, effect_val) in &effects.set {
        let resolved = resolve(effect_val, actor, wfe_id, action, input, ctx)?;
        patch.insert(field.clone(), resolved);
    }
    Ok(ctx.merge(patch))
}

fn resolve(
    val:    &EffectValue,
    actor:  &Actor,
    wfe_id: Uuid,
    action: &str,
    input:  &Value,
    ctx:    &DynCtx,
) -> Result<Value, EngineError> {
    match val {
        EffectValue::Ref { path } => resolve_ctx_ref(path, ctx),
        EffectValue::Literal(v) => {
            if let Some(s) = v.as_str() {
                Ok(resolve_special(s, actor, wfe_id, action, input))
            } else {
                Ok(v.clone())
            }
        }
    }
}

fn resolve_special(s: &str, actor: &Actor, wfe_id: Uuid, action: &str, input: &Value) -> Value {
    match s {
        "$actor" => json!({
            "orgu_id": actor.orgu_id,
            "user_id": actor.user_id,
            "role":    actor.role,
        }),
        "$timestamp" => json!(Utc::now().to_rfc3339()),
        "$wfe_id"    => json!(wfe_id),
        s if s.starts_with("$action.input.") => {
            let field = &s["$action.input.".len()..];
            input.get(field).cloned().unwrap_or(Value::Null)
        }
        _ => Value::String(s.to_string()),
    }
}

fn resolve_ctx_ref(path: &str, ctx: &DynCtx) -> Result<Value, EngineError> {
    // path: "$ctx.field.subfield"  →  ["field", "subfield"]
    let stripped = path
        .strip_prefix("$ctx.")
        .ok_or_else(|| EngineError::EffectValue(format!("ref path must start with $ctx.: {path}")))?;

    let mut current = ctx.as_value();
    for part in stripped.split('.') {
        current = current
            .get(part)
            .ok_or_else(|| EngineError::EffectValue(format!("ctx ref path not found: {path}")))?;
    }
    Ok(current.clone())
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p wfe-core engine::dynctx_apply 2>&1
```

Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/wfe-core/src/engine/dynctx_apply.rs
git commit -m "feat: wfe-core dynctx_apply — immutable effect application with special value resolution"
```

---

## Task 10: `wfe-core` — C_A Resolver

**Files:**
- Create: `crates/wfe-core/src/engine/c_a_resolver.rs`

- [ ] **Step 1: Write the test with a mock OrgPort**

```rust
// crates/wfe-core/src/engine/c_a_resolver.rs
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use uuid::Uuid;
    use serde_json::json;
    use crate::{
        error::EngineError,
        ports::OrgPort,
        types::actor::{CaRule, COrguExpr, OrgUnit},
    };

    struct MockOrg { units: Vec<OrgUnit> }

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(&self, _anchor: Uuid, _expr: &str, _orgtnt_id: Uuid)
            -> Result<Vec<OrgUnit>, EngineError>
        {
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
            -> Result<bool, EngineError>
        {
            Ok(true)
        }
    }

    fn unit(id: &str) -> OrgUnit {
        OrgUnit {
            orgu_id:   Uuid::parse_str(id).unwrap(),
            orgu_type: json!({"type": "branch"}),
            path:      "1.10.100".into(),
        }
    }

    #[tokio::test]
    async fn resolves_single_rule() {
        let orgu_id = Uuid::new_v4();
        let mock = MockOrg { units: vec![unit("00000000-0000-0000-0000-000000000001")] };
        let rule = CaRule {
            c_orgu: COrguExpr::Expr("self".into()),
            c_r:    vec![["self".into(), "clerk".into()]],
            c_u:    vec![],
        };

        let result = resolve_c_a(&[rule], orgu_id, Uuid::new_v4(), &mock).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "clerk");
    }

    #[tokio::test]
    async fn empty_rules_yields_no_candidates() {
        let mock = MockOrg { units: vec![] };
        let result = resolve_c_a(&[], Uuid::new_v4(), Uuid::new_v4(), &mock).await.unwrap();
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p wfe-core engine::c_a_resolver 2>&1 | tail -5
```

Expected: FAIL.

- [ ] **Step 3: Implement `resolve_c_a`**

```rust
// crates/wfe-core/src/engine/c_a_resolver.rs
use uuid::Uuid;
use crate::{
    error::EngineError,
    ports::OrgPort,
    types::actor::{CaRule, COrguExpr, CandidateActor},
};

/// Resolves a c_a rule array into a flat list of (orgu, role) candidate pairs.
/// Rules are OR'd — each rule independently contributes candidates.
pub async fn resolve_c_a(
    rules:          &[CaRule],
    anchor_orgu_id: Uuid,
    orgtnt_id:      Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    let mut candidates = Vec::new();
    for rule in rules {
        let orgus = resolve_c_orgu_for_rule(rule, anchor_orgu_id, orgtnt_id, org).await?;
        for unit in &orgus {
            for [_scope, role] in &rule.c_r {
                candidates.push(CandidateActor {
                    orgu_id: unit.orgu_id,
                    role:    role.clone(),
                });
            }
        }
    }
    // Deduplicate (orgu, role) pairs
    candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
    Ok(candidates)
}

/// Checks whether an actor satisfies at least one rule in the c_a array.
pub async fn actor_in_c_a(
    rules:  &[CaRule],
    actor:  &crate::types::actor::Actor,
    orgtnt_id: Uuid,
    org:    &dyn OrgPort,
) -> Result<bool, EngineError> {
    for rule in rules {
        if actor_matches_rule(rule, actor, orgtnt_id, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn actor_matches_rule(
    rule:      &CaRule,
    actor:     &crate::types::actor::Actor,
    orgtnt_id: Uuid,
    org:       &dyn OrgPort,
) -> Result<bool, EngineError> {
    let orgus = resolve_c_orgu_for_rule(rule, actor.orgu_id, orgtnt_id, org).await?;
    let actor_orgu_in_set = orgus.iter().any(|u| u.orgu_id == actor.orgu_id);
    if !actor_orgu_in_set {
        return Ok(false);
    }
    // At least one c_r pair must match: actor's role must be in the list
    // and actor must hold that role in their orgu
    for [_scope, role] in &rule.c_r {
        if role == &actor.role {
            let has_role = org.check_user_role(actor.user_id, actor.orgu_id, role).await?;
            if has_role {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn resolve_c_orgu_for_rule(
    rule:           &CaRule,
    anchor_orgu_id: Uuid,
    orgtnt_id:      Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<crate::types::actor::OrgUnit>, EngineError> {
    match &rule.c_orgu {
        COrguExpr::Expr(expr) => {
            org.resolve_c_orgu(anchor_orgu_id, expr, orgtnt_id).await
        }
        COrguExpr::Anchored { from: _, traverse } => {
            // "from" references a DynCtx field — resolved by the caller before reaching here.
            // At this stage we use anchor_orgu_id which should already be resolved.
            org.resolve_c_orgu(anchor_orgu_id, traverse, orgtnt_id).await
        }
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test -p wfe-core engine::c_a_resolver 2>&1
```

Expected: 2 tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/wfe-core/src/engine/c_a_resolver.rs
git commit -m "feat: wfe-core c_a_resolver — candidate actor resolution and actor membership check"
```

---

## Task 11: `wfe-core` — Permission + Visibility

**Files:**
- Create: `crates/wfe-core/src/engine/permission.rs`
- Create: `crates/wfe-core/src/engine/visibility.rs`

- [ ] **Step 1: Write permission test**

```rust
// Add to bottom of permission.rs (write the whole file below)
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;
    use crate::{
        engine::dynctx_apply,
        error::EngineError,
        ports::{OrgPort, WFES},
        types::{actor::{Actor, CaRule, COrguExpr, OrgUnit}, dynctx::DynCtx, wfah::Wfah,
                wfd::{ActionDef, ActionInput, Transition, WfesEffects, WftRule}, wfe::WfeStatus},
    };

    struct AlwaysMatchOrg;
    #[async_trait]
    impl OrgPort for AlwaysMatchOrg {
        async fn resolve_c_orgu(&self, anchor: Uuid, _e: &str, _t: Uuid)
            -> Result<Vec<OrgUnit>, EngineError>
        {
            Ok(vec![OrgUnit { orgu_id: anchor, orgu_type: json!({}), path: "1".into() }])
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
            -> Result<bool, EngineError> { Ok(true) }
    }

    struct NeverMatchOrg;
    #[async_trait]
    impl OrgPort for NeverMatchOrg {
        async fn resolve_c_orgu(&self, _a: Uuid, _e: &str, _t: Uuid)
            -> Result<Vec<OrgUnit>, EngineError> { Ok(vec![]) }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
            -> Result<bool, EngineError> { Ok(false) }
    }

    fn wfe_id() -> Uuid { Uuid::new_v4() }

    fn wfes(status: &str) -> WFES {
        let dynctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!(status));
            m
        });
        WFES {
            wfe_id: wfe_id(), dynctx, wfah: Wfah::empty(),
            status: WfeStatus::Active, orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(), wfd_version: 1,
        }
    }

    fn transition(action: &str, when: &str) -> Transition {
        Transition {
            id: "t1".into(), when: when.into(), action: action.into(),
            c_a: vec![CaRule {
                c_orgu: COrguExpr::Expr("self".into()),
                c_r: vec![["self".into(), "clerk".into()]],
                c_u: vec![],
            }],
            wfes_effects: WfesEffects::default(),
            trigger: None,
            wft: WftRule::Simple { c_a: vec![] },
        }
    }

    fn actor() -> Actor {
        Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: "clerk".into() }
    }

    #[tokio::test]
    async fn permitted_when_actor_matches() {
        let t = transition("approve", "$status == 'pending'");
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &AlwaysMatchOrg).await.unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn denied_when_actor_not_in_c_a() {
        let t = transition("approve", "$status == 'pending'");
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &NeverMatchOrg).await.unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn denied_when_when_condition_false() {
        let t = transition("approve", "$status == 'approved'"); // wrong status
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &AlwaysMatchOrg).await.unwrap();
        assert!(!result);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p wfe-core engine::permission 2>&1 | tail -5
```

Expected: FAIL.

- [ ] **Step 3: Implement `permission.rs`**

```rust
// crates/wfe-core/src/engine/permission.rs
use crate::{
    engine::c_a_resolver::actor_in_c_a,
    error::EngineError,
    ports::{OrgPort, WFES},
    types::{actor::Actor, wfd::Transition},
    zen,
};

/// P(WFES, Actor, ACT) → bool
/// Finds transitions matching the action + when condition, then checks actor in c_a.
pub async fn check(
    wfes:        &WFES,
    actor:       &Actor,
    action:      &str,
    transitions: &[Transition],
    org:         &dyn OrgPort,
) -> Result<bool, EngineError> {
    for t in transitions {
        if t.action != action {
            continue;
        }
        let when_matches = zen::evaluate(&t.when, wfes)?;
        if !when_matches {
            continue;
        }
        if actor_in_c_a(&t.c_a, actor, wfes.orgtnt_id, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}
```

- [ ] **Step 4: Run permission tests**

```bash
cargo test -p wfe-core engine::permission 2>&1
```

Expected: 3 tests pass.

- [ ] **Step 5: Implement `visibility.rs`**

```rust
// crates/wfe-core/src/engine/visibility.rs
use serde_json::Value;
use crate::types::{actor::Actor, dynctx::DynCtx, wfd::WFD};

/// V(DynCtx, Actor) → filtered DynCtx
/// Applies x-visibility rules from the WFD context schema.
/// Fields without x-visibility are visible by default.
pub fn apply(dynctx: &DynCtx, actor: &Actor, wfd: &WFD) -> Value {
    let schema = &wfd.context;
    let props = match schema.get("properties") {
        Some(Value::Object(p)) => p,
        _ => return dynctx.as_value().clone(),
    };

    let mut result = serde_json::Map::new();
    if let Value::Object(ctx_map) = dynctx.as_value() {
        for (field, value) in ctx_map {
            let visible = match props.get(field).and_then(|s| s.get("x-visibility")) {
                None => true,   // no visibility rule → visible by default
                Some(rule) => actor_matches_visibility(rule, actor),
            };
            if visible {
                result.insert(field.clone(), value.clone());
            }
        }
    }
    Value::Object(result)
}

fn actor_matches_visibility(rule: &Value, actor: &Actor) -> bool {
    // x-visibility: {"c_r": [["self", "manager"]], "c_orgu": "..."}
    // OR logic across criteria — if any criterion matches, field is visible.
    if let Some(c_r) = rule.get("c_r").and_then(|v| v.as_array()) {
        for pair in c_r {
            if let Some(arr) = pair.as_array() {
                let role = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                if role == actor.role {
                    return true;
                }
            }
        }
    }
    // c_orgu and c_u visibility checks are placeholders — full resolution
    // requires OrgPort which makes visibility async; for now role-based check is supported.
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use serde_json::json;

    fn actor(role: &str) -> Actor {
        Actor { orgu_id: Uuid::new_v4(), user_id: Uuid::new_v4(), role: role.into() }
    }

    #[test]
    fn no_visibility_rule_always_visible() {
        let wfd = WFD {
            id: Uuid::new_v4(), name: "t".into(), version: 1, description: None,
            context: json!({"properties": {"status": {"type": "string"}}}),
            start: vec![], actions: Default::default(), transitions: vec![],
            listable: vec![], terminal_when: "false".into(),
        };
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!("pending"));
            m
        });
        let result = apply(&ctx, &actor("clerk"), &wfd);
        assert_eq!(result["status"], json!("pending"));
    }

    #[test]
    fn visibility_rule_hides_field_for_wrong_role() {
        let wfd = WFD {
            id: Uuid::new_v4(), name: "t".into(), version: 1, description: None,
            context: json!({
                "properties": {
                    "secret": {
                        "type": "string",
                        "x-visibility": {"c_r": [["self", "manager"]]}
                    }
                }
            }),
            start: vec![], actions: Default::default(), transitions: vec![],
            listable: vec![], terminal_when: "false".into(),
        };
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("secret".into(), json!("hidden-value"));
            m
        });
        // clerk should NOT see the "secret" field
        let result = apply(&ctx, &actor("clerk"), &wfd);
        assert!(result.get("secret").is_none());

        // manager SHOULD see it
        let result2 = apply(&ctx, &actor("manager"), &wfd);
        assert_eq!(result2["secret"], json!("hidden-value"));
    }
}
```

- [ ] **Step 6: Run visibility tests**

```bash
cargo test -p wfe-core engine::visibility 2>&1
```

Expected: 2 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/wfe-core/src/engine/permission.rs crates/wfe-core/src/engine/visibility.rs
git commit -m "feat: wfe-core permission check (P function) and visibility filter (V function)"
```

---

## Task 12: `wfe-core` — Transition (apply_action)

**Files:**
- Create: `crates/wfe-core/src/engine/transition.rs`

- [ ] **Step 1: Write the tests**

```rust
// At bottom of transition.rs
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;
    use crate::{
        error::EngineError,
        ports::{OrgPort, WFES},
        types::{
            actor::{Actor, CaRule, COrguExpr, OrgUnit},
            dynctx::DynCtx,
            wfah::Wfah,
            wfd::{ActionDef, ActionInput, Transition, WfesEffects, WftRule, WFD},
            wfe::WfeStatus,
        },
    };
    use std::collections::HashMap;

    struct AlwaysMatchOrg { orgu_id: Uuid }
    #[async_trait]
    impl OrgPort for AlwaysMatchOrg {
        async fn resolve_c_orgu(&self, _a: Uuid, _e: &str, _t: Uuid)
            -> Result<Vec<OrgUnit>, EngineError>
        {
            Ok(vec![OrgUnit { orgu_id: self.orgu_id, orgu_type: json!({}), path: "1".into() }])
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
            -> Result<bool, EngineError> { Ok(true) }
    }

    fn make_wfd(terminal_when: &str) -> WFD {
        let mut actions = HashMap::new();
        actions.insert("approve".into(), ActionDef {
            name: "approve".into(), description: None,
            input: ActionInput::default(),
        });
        WFD {
            id: Uuid::new_v4(), name: "test".into(), version: 1, description: None,
            context: json!({}),
            start: vec![],
            actions,
            transitions: vec![Transition {
                id: "t1".into(),
                when: "$status == 'pending'".into(),
                action: "approve".into(),
                c_a: vec![CaRule {
                    c_orgu: COrguExpr::Expr("self".into()),
                    c_r: vec![["self".into(), "clerk".into()]],
                    c_u: vec![],
                }],
                wfes_effects: {
                    let mut e = WfesEffects::default();
                    e.set.insert("status".into(),
                        crate::types::wfd::EffectValue::Literal(json!("approved")));
                    e
                },
                trigger: None,
                wft: WftRule::Simple { c_a: vec![] },
            }],
            listable: vec![],
            terminal_when: terminal_when.into(),
        }
    }

    fn actor(orgu_id: Uuid) -> Actor {
        Actor { orgu_id, user_id: Uuid::new_v4(), role: "clerk".into() }
    }

    fn wfes(orgu_id: Uuid) -> WFES {
        let dynctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!("pending"));
            m
        });
        WFES {
            wfe_id: Uuid::new_v4(), dynctx, wfah: Wfah::empty(),
            status: WfeStatus::Active, orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(), wfd_version: 1,
        }
    }

    #[tokio::test]
    async fn apply_action_updates_dynctx() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let (new_wfes, _outcome) = apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
            .await.unwrap();
        assert_eq!(new_wfes.dynctx.get("status"), Some(&json!("approved")));
    }

    #[tokio::test]
    async fn apply_action_appends_wfah() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let (new_wfes, _outcome) = apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
            .await.unwrap();
        assert_eq!(new_wfes.wfah.entries().len(), 1);
        assert_eq!(new_wfes.wfah.entries()[0].action, "approve");
    }

    #[tokio::test]
    async fn apply_action_returns_terminal_when_condition_met() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'approved'");
        let w = wfes(orgu_id);

        let (_new_wfes, outcome) = apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
            .await.unwrap();
        assert!(matches!(outcome, WftOutcome::Terminal { .. }));
    }

    #[tokio::test]
    async fn permission_denied_returns_error() {
        struct NoMatchOrg;
        #[async_trait]
        impl OrgPort for NoMatchOrg {
            async fn resolve_c_orgu(&self, _a: Uuid, _e: &str, _t: Uuid)
                -> Result<Vec<OrgUnit>, EngineError> { Ok(vec![]) }
            async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
                -> Result<bool, EngineError> { Ok(false) }
        }
        let orgu_id = Uuid::new_v4();
        let org = NoMatchOrg;
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let err = apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
            .await.unwrap_err();
        assert!(matches!(err, EngineError::PermissionDenied(_)));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p wfe-core engine::transition 2>&1 | tail -5
```

Expected: FAIL.

- [ ] **Step 3: Implement `transition.rs`**

```rust
// crates/wfe-core/src/engine/transition.rs
use serde_json::{json, Value};
use uuid::Uuid;
use crate::{
    engine::{c_a_resolver, dynctx_apply, permission},
    error::EngineError,
    ports::{OrgPort, WFES},
    types::{
        actor::{Actor, CandidateActor},
        dynctx::DynCtx,
        wfah::Wfah,
        wfd::{WFD, WftRule, WftCondition},
        wfe::WfeStatus,
    },
    zen,
};

pub enum WftOutcome {
    NextCa(Vec<CandidateActor>),
    Terminal { end_response: Value },
}

/// WFT(WFES, Actor, ACT) → (new WFES, WftOutcome)
/// Enforces permission, applies effects, appends WFAH, evaluates terminal_when and wft.
pub async fn apply_action(
    wfes:   &WFES,
    actor:  &Actor,
    action: &str,
    input:  &Value,
    wfd:    &WFD,
    org:    &dyn OrgPort,
) -> Result<(WFES, WftOutcome), EngineError> {
    // 1. Permission check
    let permitted = permission::check(wfes, actor, action, &wfd.transitions, org).await?;
    if !permitted {
        return Err(EngineError::PermissionDenied(action.to_string()));
    }

    // 2. Find matching transition
    let transition = wfd.transitions.iter()
        .find(|t| t.action == action && zen::evaluate(&t.when, wfes).unwrap_or(false))
        .ok_or_else(|| EngineError::TransitionNotFound(action.to_string()))?;

    // 3. Apply wfes_effects → new DynCtx (immutable)
    let new_dynctx = dynctx_apply::apply(
        &wfes.dynctx, &transition.wfes_effects, actor, wfes.wfe_id, action, input
    )?;

    // 4. Append to WFAH (immutable push)
    let new_wfah = wfes.wfah.push(action.to_string(), actor.clone(), Some(input.clone()));

    // 5. Build new WFES
    let new_wfes = WFES {
        wfe_id:      wfes.wfe_id,
        dynctx:      new_dynctx,
        wfah:        new_wfah,
        status:      WfeStatus::Active,
        orgtnt_id:   wfes.orgtnt_id,
        wfd_id:      wfes.wfd_id,
        wfd_version: wfes.wfd_version,
    };

    // 6. Check terminal_when
    if zen::evaluate(&wfd.terminal_when, &new_wfes)? {
        let end_response = build_end_response(&transition.wft, &new_wfes)?;
        return Ok((new_wfes, WftOutcome::Terminal { end_response }));
    }

    // 7. Resolve wft → new C_A
    let new_c_a = resolve_wft(&transition.wft, &new_wfes, actor.orgu_id, org).await?;
    Ok((new_wfes, WftOutcome::NextCa(new_c_a)))
}

async fn resolve_wft(
    wft:            &WftRule,
    wfes:           &WFES,
    anchor_orgu_id: Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    match wft {
        WftRule::Simple { c_a } => {
            c_a_resolver::resolve_c_a(c_a, anchor_orgu_id, wfes.orgtnt_id, org).await
        }
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if zen::evaluate(&cond.when, wfes)? {
                    if cond.terminal {
                        return Ok(vec![]);
                    }
                    if let Some(c_a) = &cond.c_a {
                        return c_a_resolver::resolve_c_a(c_a, anchor_orgu_id, wfes.orgtnt_id, org).await;
                    }
                }
            }
            Ok(vec![])
        }
    }
}

fn build_end_response(wft: &WftRule, wfes: &WFES) -> Result<Value, EngineError> {
    if let WftRule::Conditional { conditions } = wft {
        for cond in conditions {
            if cond.terminal && zen::evaluate(&cond.when, wfes).unwrap_or(false) {
                if let Some(resp) = &cond.wfe_end_response {
                    return Ok(resolve_end_response_refs(resp, wfes));
                }
            }
        }
    }
    Ok(json!({}))
}

fn resolve_end_response_refs(template: &Value, wfes: &WFES) -> Value {
    match template {
        Value::Object(map) => {
            // {"ref": "$ctx.field"} → resolve from DynCtx
            if let Some(path) = map.get("ref").and_then(|v| v.as_str()) {
                if let Some(stripped) = path.strip_prefix("$ctx.") {
                    let mut current = wfes.dynctx.as_value();
                    for part in stripped.split('.') {
                        current = match current.get(part) {
                            Some(v) => v,
                            None => return Value::Null,
                        };
                    }
                    return current.clone();
                }
            }
            Value::Object(map.iter().map(|(k, v)| {
                (k.clone(), resolve_end_response_refs(v, wfes))
            }).collect())
        }
        other => other.clone(),
    }
}
```

- [ ] **Step 4: Run all wfe-core tests**

```bash
cargo test -p wfe-core 2>&1
```

Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/wfe-core/src/engine/transition.rs
git commit -m "feat: wfe-core transition engine — apply_action with permission, immutable WFES, WFT resolution"
```

---

## Task 13: Final Compilation + Full Test Run

- [ ] **Step 1: Run full workspace check**

```bash
cargo check --workspace 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 2: Run all tests in org and wfe-core**

```bash
cargo test -p wf-org -p wfe-core 2>&1
```

Expected: all unit tests pass. Integration tests in `org` (those that require DB) are skipped unless `DATABASE_URL` is set.

- [ ] **Step 3: Verify dependency graph is correct**

```bash
cargo tree -p wfe-core 2>&1 | grep "wf-org"
```

Expected: no output — `wfe-core` must NOT depend on `wf-org`.

```bash
cargo tree -p wf-org 2>&1 | grep "wfe-core"
```

Expected: no output — `wf-org` must NOT depend on `wfe-core`.

- [ ] **Step 4: Final commit**

```bash
git add -A
git commit -m "feat: Plan 1 complete — org crate + wfe-core engine foundation"
```

---

## Self-Review Checklist

### Spec Coverage

| Spec section | Task(s) |
|---|---|
| Workspace structure + 5 crates | Task 1 |
| org schema migration | Task 2 |
| wf schema migration | Task 2 |
| org models (Orgtnt, Orgt, Orgu, OrgUnit) | Task 3 |
| ORGTRVLANG parser (migrate from org-api) | Task 4 |
| ORGTRVLANG executor (migrate, schema prefix) | Task 4 |
| org repos (orgtnt, orgt, orgu) | Task 5 |
| user_role check + resolve_orgu | Task 5 |
| "*:[type:X]" global type resolution | Task 5 |
| Actor, OrgUnit, CandidateActor, CaRule, COrguExpr | Task 6 |
| DynCtx immutable (merge returns new) | Task 6 |
| Wfah append-only (push returns new) | Task 6 |
| WFD all types (Transition, WftRule, WfesEffects, EffectValue, etc.) | Task 7 |
| OrgPort, WfdPort, WfePort traits | Task 8 |
| WFES struct | Task 8 |
| ZEN wrapper (evaluate expr against WFES) | Task 8 |
| dynctx_apply ($actor, $timestamp, $wfe_id, $action.input.X, $ctx.X) | Task 9 |
| C_A resolver (resolve_c_a, actor_in_c_a) | Task 10 |
| P(WFES, Actor, ACT) permission check | Task 11 |
| V(DynCtx, Actor) visibility filter | Task 11 |
| WFT(WFES, Actor, ACT) → (new WFES, WftOutcome) | Task 12 |
| terminal_when ZEN evaluation | Task 12 |
| WftOutcome (NextCa / Terminal) | Task 12 |
| WFE_END_RESPONSE construction | Task 12 |

All spec sections covered. ✓

### Dependency Invariants

- `wfe-core` depends on no internal crates ✓
- `org` depends on no internal crates ✓
- Both verified in Task 13 with `cargo tree` ✓
