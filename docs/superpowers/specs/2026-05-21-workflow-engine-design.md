# Workflow Engine — Full System Design Spec
**Date:** 2026-05-21
**Status:** Approved
**Grand Truth:** `updated-docs/CLAUDE.md` + `updated-docs/Terminology (1).MD`

---

## 1. Goal

Implement a production-grade, enterprise workflow engine as a Rust multi-crate workspace. The engine is built around strict domain invariants from the terminology: immutable DynCtx, append-only WFAH, exact Actor tuples, ZEN-based condition evaluation, and ORGTRVLANG-based candidate actor resolution.

---

## 2. Workspace Structure

```
workflow-engine/                     ← Cargo workspace root
├── Cargo.toml                       ← [workspace] members + shared dep versions
├── .env                             ← DATABASE_URL, PORT, STORAGE_BACKEND, STORAGE_PATH
├── storage/                         ← OpenDAL local filesystem (S3 simulation)
│   └── wfd/                         ← {wfd_id}/{version}.json
├── migrations/
│   ├── org/001_initial.sql          ← org schema (migrated from init.sql)
│   └── wf/001_initial.sql           ← wf schema (wfd_meta, wfe, wfe_dynctx, wfah)
└── crates/
    ├── org/                         ← self-contained org system (no internal deps)
    ├── wfe-core/                    ← pure engine (no I/O, no framework)
    ├── wfd/                         ← WFD infrastructure adapter
    ├── wfe/                         ← WFE infrastructure adapter + execution orchestrator
    └── server/                      ← Axum HTTP binary
```

### Dependency Graph

```
server   ──► org, wfd, wfe, wfe-core
wfe      ──► wfe-core, org
wfd      ──► wfe-core
org      ──► (no internal deps)
wfe-core ──► (no internal deps)
```

### DB Schema Separation

Same PostgreSQL instance, two schemas:
- `org` schema — all org tables (`orgtnt`, `orgt`, `orgu`, `orgt_orgu`, `u`, `u_orgu`, `r`, `ur`)
- `wf` schema — all workflow tables (`wfd_meta`, `wfe`, `wfe_dynctx`, `wfah`)

---

## 3. `org` Crate — Self-Contained Org System

Independent. Can be used as a standalone org system without any other crate.

```
crates/org/
└── src/
    ├── lib.rs
    ├── error.rs
    ├── models.rs             ← Orgtnt, Orgt, Orgu, User, Role, UserRole
    ├── repo/
    │   ├── mod.rs
    │   ├── orgtnt.rs         ← list, get
    │   ├── orgt.rs           ← list_by_tenant
    │   ├── orgu.rs           ← list_by_tree, get
    │   └── user_role.rs      ← check_user_role, resolve_orgu
    ├── traversal/
    │   ├── mod.rs
    │   ├── parser.rs         ← ORGTRVLANG string → Pipeline AST (migrated from org-api)
    │   └── executor.rs       ← Pipeline + PgPool → Vec<Orgu> (migrated from org-api)
    └── adapter.rs            ← OrgAdapter: implements wfe-core::OrgPort
```

**ORGTRVLANG** lives entirely in `org`. The parser and executor are not visible to other crates. `wfe-core` passes expressions as `&str` through the `OrgPort` trait; `org` handles parsing and execution internally.

**`OrgAdapter`** implements `wfe-core::OrgPort`:
```rust
pub struct OrgAdapter { pool: PgPool }

// OrgPort impl:
// resolve_c_orgu(anchor_orgu_id, expr: &str) → Vec<OrgUnit>
//   internally: parser::parse(expr) → executor::execute(pool, anchor, orgt_id, pipeline)
// check_user_role(user_id, orgu_id, role_name) → bool
//   queries ur JOIN r where u_id=$1 AND orgu_id=$2 AND r.name=$3 AND active timeslice
```

**`user_role.rs`** — UR resolution per terminology:
- `check_user_role` evaluates `(U, ORGU, R)` against `ur` table including `valid_from`/`valid_until` timeslice
- `resolve_orgu` returns org units matching a traversal expression from a given anchor

**DB:** All queries use `SET search_path = org` or explicit `org.` prefix.

---

## 4. `wfe-core` Crate — Pure Engine

No I/O. No framework dependencies. Only: `serde`, `serde_json`, `uuid`, `chrono`, `thiserror`, `async-trait`, `zen-engine`.

```
crates/wfe-core/
└── src/
    ├── lib.rs
    ├── error.rs
    ├── types/
    │   ├── mod.rs
    │   ├── actor.rs          ← Actor, OrgUnit, CandidateActorRule, CandidateActor, CaRule
    │   ├── wfd.rs            ← WFD, Action, Transition, StartRule, AutoexecNode,
    │   │                        WftRule, WftCondition, WfesEffects, Listable, TerminalWhen
    │   ├── wfe.rs            ← WFE, WfeStatus (Active | Terminal | Error)
    │   ├── dynctx.rs         ← DynCtx (newtype over Value, immutable by design)
    │   └── wfah.rs           ← WfahEntry, Wfah (append-only, ordered by seq)
    ├── ports.rs              ← OrgPort, WfdPort, WfePort trait definitions
    ├── engine/
    │   ├── mod.rs
    │   ├── permission.rs     ← P(WFES, Actor, ACT) → bool
    │   ├── transition.rs     ← apply_action → (new WFES, Vec<CandidateActor>)
    │   ├── c_a_resolver.rs   ← resolve_c_a(CaRule, anchor, &dyn OrgPort) → Vec<CandidateActor>
    │   ├── dynctx_apply.rs   ← apply wfes_effects, produce new immutable DynCtx
    │   └── visibility.rs     ← V(DynCtx, Actor) → filtered DynCtx
    └── zen.rs                ← ZEN evaluator: exposes $<field> and $wfah from WFES
```

### Domain Types

```rust
// Actor = exact (ORGU, (U, R)) tuple — terminology invariant
pub struct Actor {
    pub orgu_id: Uuid,
    pub user_id: Uuid,
    pub role:    String,
}

// OrgUnit — minimal org info needed by the engine
pub struct OrgUnit {
    pub orgu_id:   Uuid,
    pub orgu_type: serde_json::Value,
    pub path:      String,
}

// WFES = DynCtx + WFAH — complete execution state
pub struct WFES {
    pub dynctx: DynCtx,
    pub wfah:   Wfah,
}

// DynCtx — immutable snapshot; apply_effects returns new instance, never mutates
pub struct DynCtx(serde_json::Value);
impl DynCtx {
    pub fn apply_effects(
        &self,
        effects: &WfesEffects,
        actor:   &Actor,
        wfe_id:  Uuid,
        action:  &str,
        input:   &serde_json::Value,
    ) -> Result<DynCtx, EngineError>
}

// WFAH — append-only; push returns new Wfah, never mutates existing
pub struct Wfah(Vec<WfahEntry>);
pub struct WfahEntry {
    pub seq:        u32,
    pub action:     String,
    pub actor:      Actor,
    pub input:      Option<serde_json::Value>,
    pub applied_at: DateTime<Utc>,
}
```

### WFD Types (mirrors CLAUDE.md JSON structure)

```rust
pub struct WFD {
    pub id:            Uuid,
    pub name:          String,
    pub version:       u32,
    pub description:   Option<String>,
    pub context:       serde_json::Value,   // JSON Schema 2020-12
    pub start:         Vec<StartRule>,
    pub actions:       HashMap<String, ActionDef>,
    pub transitions:   Vec<Transition>,
    pub listable:      Vec<ListableRule>,
    pub terminal_when: String,              // ZEN expression
}

pub struct StartRule {
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    pub wft:          WftRule,
}

pub struct Transition {
    pub id:           String,
    pub when:         String,           // ZEN expression
    pub action:       String,           // references actions map key
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    pub trigger:      Option<AutoexecNode>,
    pub wft:          WftRule,
}

pub struct CaRule {
    pub c_orgu: COrguExpr,            // string expr or anchored ref
    pub c_r:    Vec<[String; 2]>,     // [orgu_scope, role_name]
    pub c_u:    Option<Vec<String>>,
}

pub enum WftRule {
    Simple { c_a: Vec<CaRule> },
    Conditional { conditions: Vec<WftCondition> },
}

pub struct WftCondition {
    pub when:             String,       // ZEN expression
    pub terminal:         bool,
    pub wfe_end_response: Option<serde_json::Value>,
    pub c_a:              Option<Vec<CaRule>>,
    pub trigger:          Option<AutoexecNode>,
}
```

### Port Traits

```rust
#[async_trait]
pub trait OrgPort: Send + Sync {
    async fn resolve_c_orgu(
        &self,
        anchor_orgu_id: Uuid,
        expr: &str,
    ) -> Result<Vec<OrgUnit>, EngineError>;

    async fn check_user_role(
        &self,
        user_id: Uuid,
        orgu_id: Uuid,
        role:    &str,
    ) -> Result<bool, EngineError>;
}

#[async_trait]
pub trait WfdPort: Send + Sync {
    async fn fetch(&self, wfd_id: Uuid, version: u32) -> Result<WFD, EngineError>;
}

#[async_trait]
pub trait WfePort: Send + Sync {
    async fn load_wfes(&self, wfe_id: Uuid) -> Result<WFES, EngineError>;
    async fn persist_new_dynctx(&self, wfe_id: Uuid, ctx: &DynCtx, seq: u32) -> Result<(), EngineError>;
    async fn append_wfah(&self, wfe_id: Uuid, entry: &WfahEntry) -> Result<(), EngineError>;
    async fn update_c_a(&self, wfe_id: Uuid, c_a: &[CandidateActor]) -> Result<(), EngineError>;
    async fn set_terminal(&self, wfe_id: Uuid, end_response: &serde_json::Value) -> Result<(), EngineError>;
}
```

### Engine Functions

```rust
// core/engine/transition.rs
pub async fn apply_action(
    wfes:   &WFES,
    actor:  &Actor,
    action: &str,
    input:  &serde_json::Value,
    wfd:    &WFD,
    org:    &dyn OrgPort,
) -> Result<(WFES, WftOutcome), EngineError>

// core/engine/permission.rs
pub async fn check_permission(
    wfes:   &WFES,
    actor:  &Actor,
    action: &str,
    wfd:    &WFD,
    org:    &dyn OrgPort,
) -> Result<bool, EngineError>

// core/engine/c_a_resolver.rs
pub async fn resolve_c_a(
    rules:          &[CaRule],
    anchor_orgu_id: Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError>
```

### ZEN Evaluator

```rust
// zen.rs
pub fn evaluate(expr: &str, wfes: &WFES) -> Result<bool, EngineError>
// Builds ZEN context: { "$field": value, ..., "$wfah": [...] }
// All DynCtx top-level fields exposed as $field
// Full WFAH array exposed as $wfah: [{action, actor, applied_at}]
```

---

## 5. `wfd` Crate — WFD Infrastructure Adapter

```
crates/wfd/
└── src/
    ├── lib.rs
    ├── error.rs
    ├── models.rs             ← WfdMeta (DB row)
    ├── storage.rs            ← OpenDAL Operator factory
    ├── repo.rs               ← wf.wfd_meta CRUD (sqlx)
    └── adapter.rs            ← WfdAdapter: implements wfe-core::WfdPort
```

**DB table:**
```sql
CREATE TABLE wf.wfd_meta (
    wfd_id      uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id   uuid        NOT NULL,
    name        text        NOT NULL,
    version     integer     NOT NULL DEFAULT 1,
    s3_key      text        NOT NULL,   -- "wfd/{wfd_id}/{version}.json"
    is_active   boolean     NOT NULL DEFAULT true,
    created_at  timestamptz NOT NULL DEFAULT now(),
    UNIQUE (orgtnt_id, name, version)
);
```

**Storage config** (from `.env`):
```
STORAGE_BACKEND=local          # or: s3
STORAGE_PATH=./storage         # local path (ignored for s3)
STORAGE_S3_BUCKET=...          # s3 only
STORAGE_S3_REGION=...          # s3 only
```

**`WfdAdapter::fetch`** flow:
1. Query `wf.wfd_meta` for `s3_key` by `(wfd_id, version)`
2. Read JSON bytes from OpenDAL operator at `s3_key`
3. `serde_json::from_slice::<WFD>` → return

**HTTP endpoints** (in server):
```
POST   /wfd              ← upload new WFD (JSON body validated against WFD struct)
GET    /wfd              ← list (orgtnt_id query param)
GET    /wfd/:id/:version ← fetch single WFD
PUT    /wfd/:id          ← upload new version (increments version)
```

---

## 6. `wfe` Crate — WFE Infrastructure Adapter + Execution Orchestrator

```
crates/wfe/
└── src/
    ├── lib.rs
    ├── error.rs
    ├── models.rs             ← WfeRow, DynCtxRow, WfahRow
    ├── repo/
    │   ├── mod.rs
    │   ├── wfe.rs            ← wf.wfe CRUD
    │   ├── dynctx.rs         ← wf.wfe_dynctx insert + load latest
    │   └── wfah.rs           ← wf.wfah append + load all
    ├── adapter.rs            ← WfeAdapter: implements wfe-core::WfePort
    └── executor.rs           ← WfeExecutor: orchestrates engine + adapters
```

**DB tables:**
```sql
CREATE TABLE wf.wfe (
    wfe_id       uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    orgtnt_id    uuid        NOT NULL,
    wfd_id       uuid        NOT NULL,
    wfd_version  integer     NOT NULL,
    status       text        NOT NULL CHECK (status IN ('active','terminal','error')),
    current_c_a  jsonb       NOT NULL,   -- cached for efficient actor queries
    end_response jsonb,                  -- populated on terminal
    created_at   timestamptz NOT NULL DEFAULT now(),
    updated_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE wf.wfe_dynctx (
    dynctx_id  uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    ctx        jsonb       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);

CREATE TABLE wf.wfah (
    wfah_id    uuid        PRIMARY KEY DEFAULT uuid_generate_v4(),
    wfe_id     uuid        NOT NULL REFERENCES wf.wfe(wfe_id),
    seq        integer     NOT NULL,
    action     text        NOT NULL,
    actor      jsonb       NOT NULL,   -- {orgu_id, user_id, role}
    input      jsonb,
    applied_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (wfe_id, seq)
);

CREATE INDEX wfe_orgtnt_idx      ON wf.wfe(orgtnt_id);
CREATE INDEX wfe_status_idx      ON wf.wfe(status);
CREATE INDEX wfe_dynctx_wfe_idx  ON wf.wfe_dynctx(wfe_id);
CREATE INDEX wfah_wfe_idx        ON wf.wfah(wfe_id);
```

**`WfeExecutor`** — ties everything together:
```rust
pub struct WfeExecutor {
    org: Arc<dyn OrgPort>,
    wfd: Arc<dyn WfdPort>,
    wfe: Arc<dyn WfePort>,
}

impl WfeExecutor {
    pub async fn start(
        &self,
        wfd_id:    Uuid,
        version:   u32,
        actor:     &Actor,
        input:     &serde_json::Value,
    ) -> Result<WfeStartResult>

    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
        action: &str,
        input:  &serde_json::Value,
    ) -> Result<WfeApplyResult>

    pub async fn query(
        &self,
        wfe_id: Uuid,
        viewer: &Actor,
    ) -> Result<WfeView>   // V(DynCtx, viewer) applied

    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
    ) -> Result<Vec<String>>

    pub async fn list(
        &self,
        orgtnt_id: Uuid,
        actor:     &Actor,
    ) -> Result<Vec<WfeSummary>>  // listable check applied
}
```

**`apply` execution flow** (maps exactly to terminology):
1. Load `WFES` (latest `DynCtx` + full `WFAH`) via `WfePort`
2. Fetch `WFD` via `WfdPort`
3. Find matching transition: `when` ZEN expression evaluated against `WFES`
4. `check_permission(WFES, Actor, ACT, WFD, OrgPort)` → 403 if false
5. `new_dynctx = dynctx.apply_effects(effects, actor, wfe_id, action, input)`
6. `new_wfah = wfah.push(WfahEntry { action, actor, input, seq: next })`
7. `new_wfes = WFES { dynctx: new_dynctx, wfah: new_wfah }`
8. Evaluate `terminal_when` ZEN against `new_wfes` → if true, set terminal + store `end_response`
9. Else: `WFT(new_wfes, actor, action)` → resolve new `C_A` via `OrgPort`
10. Persist: `WfePort::persist_new_dynctx`, `append_wfah`, `update_c_a` (or `set_terminal`)
11. Fire `trigger` if present (autoexec — deferred to later sprint)

**HTTP endpoints** (in server):
```
POST   /wfe                          ← start WFE
POST   /wfe/:id/actions              ← apply action {actor, action, input}
GET    /wfe/:id                      ← query state (actor from header)
GET    /wfe/:id/possible-actions     ← P_ACT_A(WFES, A)
GET    /wfe                          ← list (listable check, orgtnt filter)
```

---

## 7. `server` Crate — HTTP Binary

```
crates/server/
└── src/
    ├── main.rs           ← wire-up: pool, adapters, executor, router, serve
    ├── config.rs         ← Config from .env
    ├── error.rs          ← AppError → HTTP status + JSON body
    ├── state.rs          ← AppState (Arc<WfeExecutor>, PgPool, Arc<WfdAdapter>)
    └── routes/
        ├── mod.rs
        ├── org.rs        ← /org/* handlers (uses org::repo directly)
        ├── wfd.rs        ← /wfd/* handlers (uses WfdAdapter + wfd::repo)
        └── wfe.rs        ← /wfe/* handlers (delegates to WfeExecutor)
```

**Wire-up:**
```rust
let pool         = PgPoolOptions::new().connect(&cfg.database_url).await?;
let storage_op   = wfd::storage::build_operator(&cfg.storage)?;
let org_adapter  = Arc::new(org::OrgAdapter::new(pool.clone()));
let wfd_adapter  = Arc::new(wfd::WfdAdapter::new(pool.clone(), storage_op));
let wfe_adapter  = Arc::new(wfe::WfeAdapter::new(pool.clone()));
let executor     = Arc::new(WfeExecutor::new(org_adapter, wfd_adapter.clone(), wfe_adapter));
```

**Actor extraction:** Actor `(orgu_id, user_id, role)` passed as JSON header `X-Actor` or request body field. No auth in this sprint — validated structurally only.

---

## 8. Shared Cargo.toml (Workspace)

```toml
[workspace]
members = ["crates/org", "crates/wfe-core", "crates/wfd", "crates/wfe", "crates/server"]
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

---

## 9. Key Design Invariants (from Terminology)

1. `DynCtx` is **immutable** — `apply_effects` always returns a new instance; `wfe_dynctx` table is insert-only
2. `Actor` is an **exact** `(ORGU, (U, R))` triple — no partial matching anywhere
3. `Permission` is an **exact** evaluation against current `WFES` — no ambiguity
4. `WFAH` is **append-only** — `seq` is monotonically increasing, never updated
5. `WFD` must always be available alongside its `WFE` — `wfd_id + wfd_version` stored on every `wfe` row
6. `ORGTRVLANG` lives entirely in `org` crate — other crates pass expressions as `&str`
7. ZEN evaluates all `when` conditions — no custom predicate format

---

## 10. Out of Scope (This Sprint)

- Autoexec node execution (`rest`, `sql`, `calc`) — types defined in `wfe-core`, execution deferred
- TRIGGER firing — type defined, execution deferred
- Authentication / JWT — `X-Actor` header used structurally
- Write mutations for org layer (users, roles, orgu CRUD) — read-only org API
