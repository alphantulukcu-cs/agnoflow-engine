# Org API — Design Spec
**Date:** 2026-05-20  
**Status:** Approved  
**Goal:** Developer/test tool — REST API for the WF Engine Organization layer and ORGTRVLANG traversal language.

---

## Scope

- Read-only REST API (no auth, no create/update/delete)
- Axum + SQLx + PostgreSQL (ltree extension)
- Exposes the 9 ORGTRVLANG tokens via a single `?expr=` endpoint
- Uses the existing `init.sql` schema and SQL functions exactly as defined

Out of scope: WFD/WFE workflow layer, user-role management, mutations.

---

## Project Structure

```
wf-engine-org-api/
├── Cargo.toml
├── .env                          ← DATABASE_URL, PORT
└── src/
    ├── main.rs                   ← router setup, pool init, server start
    ├── config.rs                 ← env reading (DATABASE_URL, PORT)
    ├── error.rs                  ← AppError → IntoResponse (thiserror)
    ├── models.rs                 ← Orgtnt, Orgt, Orgu domain types
    ├── handlers/
    │   ├── mod.rs                ← re-exports
    │   ├── orgtnt.rs             ← list_orgtnt, get_orgtnt
    │   ├── orgt.rs               ← list_orgt_by_tenant
    │   ├── orgu.rs               ← list_orgu_by_tree, get_orgu
    │   └── traverse.rs           ← traverse_orgu (parses expr, calls executor)
    └── traversal/
        ├── mod.rs                ← re-exports TraversalExpr, parse, execute
        ├── parser.rs             ← DSL string → TraversalExpr enum
        └── executor.rs           ← TraversalExpr → sqlx query → Vec<Orgu>
```

---

## Dependencies (`Cargo.toml`)

```toml
[dependencies]
axum          = "0.7"
tokio         = { version = "1", features = ["full"] }
sqlx          = { version = "0.7", features = ["postgres", "runtime-tokio-rustls", "uuid", "chrono", "json"] }
serde         = { version = "1", features = ["derive"] }
serde_json    = "1"
thiserror     = "1"
tracing       = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
dotenvy       = "0.15"
uuid          = { version = "1", features = ["serde"] }
```

---

## Domain Models (`models.rs`)

All types derive `Serialize, sqlx::FromRow`.  
`path` (ltree) is mapped as `String` via `TEXT` cast in all queries.  
`metadata` is `Option<serde_json::Value>` mapped via `JSONB`.

```rust
pub struct Orgtnt {
    pub orgtnt_id: Uuid,
    pub name:      String,
    pub code:      String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct Orgt {
    pub orgt_id:     Uuid,
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub description: Option<String>,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

pub struct Orgu {
    pub orgu_id:        Uuid,
    pub orgtnt_id:      Uuid,
    pub orgt_id:        Uuid,
    pub parent_orgu_id: Option<Uuid>,
    pub path:           String,          // ltree → TEXT
    pub orgu_t:         String,
    pub name:           String,
    pub metadata:       Option<serde_json::Value>,
    pub is_active:      bool,
    pub created_at:     DateTime<Utc>,
    pub updated_at:     DateTime<Utc>,
}
```

---

## Endpoints

| Method | Path                          | Handler               | Description                        |
|--------|-------------------------------|-----------------------|------------------------------------|
| GET    | `/orgtnt`                     | `list_orgtnt`         | All active tenants                 |
| GET    | `/orgtnt/:id`                 | `get_orgtnt`          | Single tenant by UUID              |
| GET    | `/orgtnt/:id/orgt`            | `list_orgt_by_tenant` | Trees belonging to a tenant        |
| GET    | `/orgt/:id/orgu`              | `list_orgu_by_tree`   | All ORGU nodes in a tree (flat)    |
| GET    | `/orgu/:id`                   | `get_orgu`            | Single ORGU by UUID                |
| GET    | `/orgu/:id/traverse?expr=...` | `traverse_orgu`       | ORGTRVLANG traversal               |

All responses: `Content-Type: application/json`.  
All errors: `{ "error": "<message>" }` with appropriate HTTP status.

### Traverse endpoint detail

`GET /orgu/:id/traverse?expr=<ORGTRVLANG expression>`

- `id` is the anchor ORGU UUID
- `expr` is a URL-encoded ORGTRVLANG expression (see below)
- `orgt_id` is **not** a parameter — it is read from the ORGU record automatically

**Example calls:**
```
GET /orgu/{uuid}/traverse?expr=self
GET /orgu/{uuid}/traverse?expr=parent
GET /orgu/{uuid}/traverse?expr=siblings
GET /orgu/{uuid}/traverse?expr=siblings[sube]
GET /orgu/{uuid}/traverse?expr=children
GET /orgu/{uuid}/traverse?expr=children[il]
GET /orgu/{uuid}/traverse?expr=up[bolge]
GET /orgu/{uuid}/traverse?expr=up[bolge].children
GET /orgu/{uuid}/traverse?expr=up[bolge].children[il]
GET /orgu/{uuid}/traverse?expr=children[il].children[sube]
```

**Response:** `200 OK` with `Vec<Orgu>` JSON array, or `400 Bad Request` with error message for unknown expressions.

---

## Traversal Parser (`traversal/parser.rs`)

### TraversalExpr enum

```rust
pub enum TraversalExpr {
    Self_,
    Parent,
    Siblings,
    SiblingsT(String),
    Children,
    ChildrenT(String),
    UpT(String),
    UpTChildren(String),
    UpTChildrenT { ancestor_type: String, child_type: String },
    ChildrenTChildrenT { parent_type: String, child_type: String },
}
```

### Parsing rules (hand-written, no regex)

| Input pattern            | TraversalExpr variant                        |
|--------------------------|----------------------------------------------|
| `self`                   | `Self_`                                      |
| `parent`                 | `Parent`                                     |
| `siblings`               | `Siblings`                                   |
| `siblings[T]`            | `SiblingsT("T")`                             |
| `children`               | `Children`                                   |
| `children[T]`            | `ChildrenT("T")`                             |
| `up[T]`                  | `UpT("T")`                                   |
| `up[T].children`         | `UpTChildren("T")`                           |
| `up[T].children[T2]`     | `UpTChildrenT { ancestor: T, child: T2 }`    |
| `children[T].children[T2]` | `ChildrenTChildrenT { parent: T, child: T2 }` |

Unknown patterns return `Err(ParseError::UnknownExpression)` → HTTP 400.

---

## Traversal Executor (`traversal/executor.rs`)

Each `TraversalExpr` variant dispatches to the corresponding SQL function from `init.sql`:

| TraversalExpr variant       | SQL function called                      |
|-----------------------------|------------------------------------------|
| `Self_`                     | `orgtrvlang_self($1, $2)`               |
| `Parent`                    | `orgtrvlang_parent($1, $2)`             |
| `Siblings`                  | `orgtrvlang_siblings($1, $2)`           |
| `SiblingsT(T)`              | `orgtrvlang_siblings_t($1, $2, $3)`     |
| `Children`                  | `orgtrvlang_children($1, $2)`           |
| `ChildrenT(T)`              | `orgtrvlang_children_t($1, $2, $3)`     |
| `UpT(T)`                    | `orgtrvlang_up_t($1, $2, $3)`           |
| `UpTChildren(T)`            | `orgtrvlang_up_t_children($1, $2, $3)`  |
| `UpTChildrenT {..}`         | `orgtrvlang_up_t_children_t($1,$2,$3,$4)` |
| `ChildrenTChildrenT {..}`   | `orgtrvlang_children_t_children_t($1,$2,$3,$4)` |

`$1` = anchor `orgu_id`, `$2` = `orgt_id` (looked up from anchor), `$3`/`$4` = type strings.

All queries cast `path` as `TEXT`: `SELECT orgu_id, orgtnt_id, orgt_id, parent_orgu_id, path::text AS path, ...`

---

## Error Handling (`error.rs`)

```rust
pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Database(sqlx::Error),
}
```

- `NotFound` → `404 { "error": "..." }`
- `BadRequest` → `400 { "error": "..." }`
- `Database` → `500 { "error": "internal database error" }` (details only in tracing log)

---

## Config (`config.rs`)

Read from environment / `.env` via `dotenvy`:
- `DATABASE_URL` (required) — PostgreSQL connection string
- `PORT` (default: `3000`)

---

## Test Data (pre-loaded by init.sql)

Anchor for traversal testing: **Çankaya Şubesi** `orgu_id = 00000000-0000-0000-0001-000000000004`, `path = 1.2.3.4`

Full test matrix documented in `init.sql` section 5 with expected results for all 9 tokens.

---

## What is NOT in scope

- Authentication / authorization
- Create / update / delete endpoints
- WFD/WFE layer
- User-role assignment
- Pagination (all list endpoints return all rows — test data is small)
