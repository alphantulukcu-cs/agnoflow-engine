# Org CRUD Backend — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add full CRUD endpoints for users (`u`), roles (`r`), user-role assignments (`ur`), and org unit mutations (`orgu` create/update/delete) to the existing `wf-server` Axum binary.

**Architecture:** Extend `crates/org/src/models.rs` with new types, add three new repo modules (`user`, `role`, `ur`) and mutation functions to the existing `orgtnt`/`orgt`/`orgu` repos, then wire all new handlers into `crates/server/src/routes/org.rs`. No new crates, no migrations — all tables already exist in the `org` schema.

**Tech Stack:** Rust, Axum 0.7, sqlx 0.7, PostgreSQL (`org` schema). Tables: `org.u`, `org.u_orgu`, `org.r`, `org.ur`, `org.orgu`, `org.orgt_orgu`, `org.orgtnt`, `org.orgt`.

---

## File Map

```
crates/org/src/
├── models.rs                   MODIFY — add User, Role, UR, UOrgu structs
└── repo/
    ├── mod.rs                  MODIFY — pub mod user; pub mod role; pub mod ur;
    ├── orgtnt.rs               MODIFY — add create, update
    ├── orgt.rs                 MODIFY — add create, update
    ├── orgu.rs                 MODIFY — add create, update, delete
    ├── user.rs                 CREATE — list, get, create, update, delete
    ├── role.rs                 CREATE — list, get, create, update, delete
    └── ur.rs                   CREATE — list, create, delete

crates/server/src/routes/
└── org.rs                      MODIFY — add all CRUD handlers
```

---

## Task 1: Add User, Role, UR models to `org` crate

**Files:**
- Modify: `crates/org/src/models.rs`

- [ ] **Step 1: Append new model types to models.rs**

Add after the existing `OrgUnit` impl at the end of the file:

```rust
// --- User (u table) ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct User {
    pub u_id:       Uuid,
    pub orgtnt_id:  Uuid,
    pub username:   String,
    pub full_name:  String,
    pub email:      Option<String>,
    pub is_active:  bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub orgtnt_id: Uuid,
    pub username:  String,
    pub full_name: String,
    pub email:     Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUser {
    pub full_name: Option<String>,
    pub email:     Option<String>,
    pub is_active: Option<bool>,
}

// --- UOrgu (u_orgu table) ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UOrgu {
    pub u_orgu_id:  Uuid,
    pub orgtnt_id:  Uuid,
    pub u_id:       Uuid,
    pub orgu_id:    Uuid,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

// --- Role (r table) ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Role {
    pub r_id:         Uuid,
    pub orgtnt_id:    Uuid,
    pub name:         String,
    pub display_name: String,
    pub is_active:    bool,
    pub created_at:   DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateRole {
    pub orgtnt_id:    Uuid,
    pub name:         String,
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateRole {
    pub display_name: Option<String>,
    pub is_active:    Option<bool>,
}

// --- UR (ur table) ---

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct Ur {
    pub ur_id:      Uuid,
    pub orgtnt_id:  Uuid,
    pub u_id:       Uuid,
    pub r_id:       Uuid,
    pub orgu_id:    Option<Uuid>,
    pub orgu_scope: Option<String>,
    pub ur_type:    String,
    pub valid_from:  Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at:  DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUr {
    pub orgtnt_id:  Uuid,
    pub u_id:       Uuid,
    pub r_id:       Uuid,
    pub orgu_id:    Option<Uuid>,
    pub orgu_scope: Option<String>,
    pub ur_type:    Option<String>,     // defaults to "granted"
    pub valid_from:  Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

// --- Org mutation inputs ---

#[derive(Debug, Deserialize)]
pub struct CreateOrgtnt {
    pub name: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOrgtnt {
    pub name:      Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateOrgt {
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOrgt {
    pub name:        Option<String>,
    pub description: Option<String>,
    pub is_active:   Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreateOrgu {
    pub orgt_id:        Uuid,
    pub orgtnt_id:      Uuid,
    pub name:           String,
    pub orgu_type:      serde_json::Value,
    pub parent_orgu_id: Option<Uuid>,
    pub metadata:       Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateOrgu {
    pub name:      Option<String>,
    pub metadata:  Option<serde_json::Value>,
    pub is_active: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct AssignUOrgu {
    pub u_id:       Uuid,
    pub orgu_id:    Uuid,
    pub orgtnt_id:  Uuid,
    pub is_primary: Option<bool>,
}
```

Also add `Deserialize` to the serde import at the top of models.rs — change:
```rust
use serde::Serialize;
```
to:
```rust
use serde::{Deserialize, Serialize};
```

- [ ] **Step 2: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/org/src/models.rs
git commit -m "feat(org): add User, Role, Ur, mutation input models"
```

---

## Task 2: Extend `orgtnt` and `orgt` repos with create/update

**Files:**
- Modify: `crates/org/src/repo/orgtnt.rs`
- Modify: `crates/org/src/repo/orgt.rs`

- [ ] **Step 1: Add create/update to `orgtnt.rs`**

Append to existing `crates/org/src/repo/orgtnt.rs`:

```rust
use crate::models::{CreateOrgtnt, UpdateOrgtnt};

pub async fn create(pool: &PgPool, body: &CreateOrgtnt) -> Result<Orgtnt, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "INSERT INTO org.orgtnt (name, code)
         VALUES ($1, $2)
         RETURNING orgtnt_id, name, code, is_active, created_at, updated_at"
    )
    .bind(&body.name)
    .bind(&body.code)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(pool: &PgPool, id: Uuid, body: &UpdateOrgtnt) -> Result<Orgtnt, OrgError> {
    sqlx::query_as::<_, Orgtnt>(
        "UPDATE org.orgtnt
         SET name      = COALESCE($1, name),
             is_active = COALESCE($2, is_active),
             updated_at = now()
         WHERE orgtnt_id = $3
         RETURNING orgtnt_id, name, code, is_active, created_at, updated_at"
    )
    .bind(&body.name)
    .bind(body.is_active)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}
```

- [ ] **Step 2: Add create/update to `orgt.rs`**

Append to existing `crates/org/src/repo/orgt.rs`:

```rust
use crate::models::{CreateOrgt, UpdateOrgt};

pub async fn create(pool: &PgPool, body: &CreateOrgt) -> Result<Orgt, OrgError> {
    sqlx::query_as::<_, Orgt>(
        "INSERT INTO org.orgt (orgtnt_id, name, description)
         VALUES ($1, $2, $3)
         RETURNING orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at"
    )
    .bind(body.orgtnt_id)
    .bind(&body.name)
    .bind(&body.description)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(pool: &PgPool, id: Uuid, body: &UpdateOrgt) -> Result<Orgt, OrgError> {
    sqlx::query_as::<_, Orgt>(
        "UPDATE org.orgt
         SET name        = COALESCE($1, name),
             description = COALESCE($2, description),
             is_active   = COALESCE($3, is_active),
             updated_at  = now()
         WHERE orgt_id = $4
         RETURNING orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at"
    )
    .bind(&body.name)
    .bind(&body.description)
    .bind(body.is_active)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgt {id}")))
}
```

- [ ] **Step 3: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/org/src/repo/orgtnt.rs crates/org/src/repo/orgt.rs
git commit -m "feat(org): orgtnt and orgt create/update repo functions"
```

---

## Task 3: Extend `orgu` repo with create/update/delete

**Files:**
- Modify: `crates/org/src/repo/orgu.rs`

- [ ] **Step 1: Append to `crates/org/src/repo/orgu.rs`**

```rust
use crate::models::{CreateOrgu, UpdateOrgu};

/// Inserts into org.orgu and org.orgt_orgu atomically.
/// path is computed as: parent_path.new_orgu_id (or just new_orgu_id at root)
pub async fn create(pool: &PgPool, body: &CreateOrgu) -> Result<Orgu, OrgError> {
    let orgu_id = Uuid::new_v4();

    // Compute path
    let path: String = match body.parent_orgu_id {
        None => orgu_id.simple().to_string(),
        Some(parent_id) => {
            let parent_path: String = sqlx::query_scalar(
                "SELECT path::text FROM org.orgt_orgu
                 WHERE orgu_id = $1 AND orgt_id = $2"
            )
            .bind(parent_id)
            .bind(body.orgt_id)
            .fetch_optional(pool)
            .await?
            .ok_or_else(|| OrgError::NotFound(format!("parent orgu {parent_id} in orgt {}", body.orgt_id)))?;
            format!("{}.{}", parent_path, orgu_id.simple())
        }
    };

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO org.orgu (orgu_id, orgu_type, name, metadata)
         VALUES ($1, $2, $3, $4)"
    )
    .bind(orgu_id)
    .bind(&body.orgu_type)
    .bind(&body.name)
    .bind(&body.metadata)
    .execute(&mut *tx)
    .await
    .map_err(OrgError::Database)?;

    sqlx::query(
        "INSERT INTO org.orgt_orgu (orgt_id, orgu_id, orgtnt_id, parent_orgu_id, path)
         VALUES ($1, $2, $3, $4, $5::ltree)"
    )
    .bind(body.orgt_id)
    .bind(orgu_id)
    .bind(body.orgtnt_id)
    .bind(body.parent_orgu_id)
    .bind(&path)
    .execute(&mut *tx)
    .await
    .map_err(OrgError::Database)?;

    tx.commit().await?;

    get(pool, orgu_id).await
}

pub async fn update(pool: &PgPool, orgu_id: Uuid, body: &UpdateOrgu) -> Result<Orgu, OrgError> {
    sqlx::query(
        "UPDATE org.orgu
         SET name       = COALESCE($1, name),
             metadata   = COALESCE($2, metadata),
             is_active  = COALESCE($3, is_active),
             updated_at = now()
         WHERE orgu_id = $4"
    )
    .bind(&body.name)
    .bind(&body.metadata)
    .bind(body.is_active)
    .bind(orgu_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;

    get(pool, orgu_id).await
}

/// Soft delete: sets is_active = false on org.orgu and org.orgt_orgu rows.
pub async fn delete(pool: &PgPool, orgu_id: Uuid) -> Result<(), OrgError> {
    sqlx::query(
        "UPDATE org.orgu SET is_active = false, updated_at = now() WHERE orgu_id = $1"
    )
    .bind(orgu_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;

    sqlx::query(
        "UPDATE org.orgt_orgu SET is_active = false, updated_at = now() WHERE orgu_id = $1"
    )
    .bind(orgu_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;

    Ok(())
}
```

- [ ] **Step 2: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/org/src/repo/orgu.rs
git commit -m "feat(org): orgu create/update/soft-delete repo functions"
```

---

## Task 4: Create `user` repo

**Files:**
- Create: `crates/org/src/repo/user.rs`

- [ ] **Step 1: Create `crates/org/src/repo/user.rs`**

```rust
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::{AssignUOrgu, CreateUser, UOrgu, UpdateUser, User}};

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<User>, OrgError> {
    sqlx::query_as::<_, User>(
        "SELECT u_id, orgtnt_id, username, full_name, email, is_active, created_at
         FROM org.u WHERE orgtnt_id = $1 ORDER BY full_name"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, u_id: Uuid) -> Result<User, OrgError> {
    sqlx::query_as::<_, User>(
        "SELECT u_id, orgtnt_id, username, full_name, email, is_active, created_at
         FROM org.u WHERE u_id = $1"
    )
    .bind(u_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("user {u_id}")))
}

pub async fn create(pool: &PgPool, body: &CreateUser) -> Result<User, OrgError> {
    sqlx::query_as::<_, User>(
        "INSERT INTO org.u (orgtnt_id, username, full_name, email)
         VALUES ($1, $2, $3, $4)
         RETURNING u_id, orgtnt_id, username, full_name, email, is_active, created_at"
    )
    .bind(body.orgtnt_id)
    .bind(&body.username)
    .bind(&body.full_name)
    .bind(&body.email)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(pool: &PgPool, u_id: Uuid, body: &UpdateUser) -> Result<User, OrgError> {
    sqlx::query_as::<_, User>(
        "UPDATE org.u
         SET full_name = COALESCE($1, full_name),
             email     = COALESCE($2, email),
             is_active = COALESCE($3, is_active)
         WHERE u_id = $4
         RETURNING u_id, orgtnt_id, username, full_name, email, is_active, created_at"
    )
    .bind(&body.full_name)
    .bind(&body.email)
    .bind(body.is_active)
    .bind(u_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("user {u_id}")))
}

pub async fn delete(pool: &PgPool, u_id: Uuid) -> Result<(), OrgError> {
    sqlx::query("UPDATE org.u SET is_active = false WHERE u_id = $1")
        .bind(u_id)
        .execute(pool)
        .await
        .map_err(OrgError::Database)?;
    Ok(())
}

pub async fn assign_orgu(pool: &PgPool, body: &AssignUOrgu) -> Result<UOrgu, OrgError> {
    sqlx::query_as::<_, UOrgu>(
        "INSERT INTO org.u_orgu (orgtnt_id, u_id, orgu_id, is_primary)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (u_id, orgu_id) DO UPDATE SET is_primary = EXCLUDED.is_primary
         RETURNING u_orgu_id, orgtnt_id, u_id, orgu_id, is_primary, created_at"
    )
    .bind(body.orgtnt_id)
    .bind(body.u_id)
    .bind(body.orgu_id)
    .bind(body.is_primary.unwrap_or(false))
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_orgus(pool: &PgPool, u_id: Uuid) -> Result<Vec<UOrgu>, OrgError> {
    sqlx::query_as::<_, UOrgu>(
        "SELECT u_orgu_id, orgtnt_id, u_id, orgu_id, is_primary, created_at
         FROM org.u_orgu WHERE u_id = $1"
    )
    .bind(u_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}
```

- [ ] **Step 2: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

---

## Task 5: Create `role` and `ur` repos

**Files:**
- Create: `crates/org/src/repo/role.rs`
- Create: `crates/org/src/repo/ur.rs`

- [ ] **Step 1: Create `crates/org/src/repo/role.rs`**

```rust
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::{CreateRole, Role, UpdateRole}};

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Role>, OrgError> {
    sqlx::query_as::<_, Role>(
        "SELECT r_id, orgtnt_id, name, display_name, is_active, created_at
         FROM org.r WHERE orgtnt_id = $1 ORDER BY name"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, r_id: Uuid) -> Result<Role, OrgError> {
    sqlx::query_as::<_, Role>(
        "SELECT r_id, orgtnt_id, name, display_name, is_active, created_at
         FROM org.r WHERE r_id = $1"
    )
    .bind(r_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("role {r_id}")))
}

pub async fn create(pool: &PgPool, body: &CreateRole) -> Result<Role, OrgError> {
    sqlx::query_as::<_, Role>(
        "INSERT INTO org.r (orgtnt_id, name, display_name)
         VALUES ($1, $2, $3)
         RETURNING r_id, orgtnt_id, name, display_name, is_active, created_at"
    )
    .bind(body.orgtnt_id)
    .bind(&body.name)
    .bind(&body.display_name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(pool: &PgPool, r_id: Uuid, body: &UpdateRole) -> Result<Role, OrgError> {
    sqlx::query_as::<_, Role>(
        "UPDATE org.r
         SET display_name = COALESCE($1, display_name),
             is_active    = COALESCE($2, is_active)
         WHERE r_id = $3
         RETURNING r_id, orgtnt_id, name, display_name, is_active, created_at"
    )
    .bind(&body.display_name)
    .bind(body.is_active)
    .bind(r_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("role {r_id}")))
}

pub async fn delete(pool: &PgPool, r_id: Uuid) -> Result<(), OrgError> {
    sqlx::query("UPDATE org.r SET is_active = false WHERE r_id = $1")
        .bind(r_id)
        .execute(pool)
        .await
        .map_err(OrgError::Database)?;
    Ok(())
}
```

- [ ] **Step 2: Create `crates/org/src/repo/ur.rs`**

```rust
use sqlx::PgPool;
use uuid::Uuid;
use crate::{error::OrgError, models::{CreateUr, Ur}};

pub async fn list(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Ur>, OrgError> {
    sqlx::query_as::<_, Ur>(
        "SELECT ur_id, orgtnt_id, u_id, r_id, orgu_id, orgu_scope,
                ur_type, valid_from, valid_until, created_at
         FROM org.ur WHERE orgtnt_id = $1 ORDER BY created_at DESC"
    )
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn create(pool: &PgPool, body: &CreateUr) -> Result<Ur, OrgError> {
    sqlx::query_as::<_, Ur>(
        "INSERT INTO org.ur (orgtnt_id, u_id, r_id, orgu_id, orgu_scope,
                             ur_type, valid_from, valid_until)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING ur_id, orgtnt_id, u_id, r_id, orgu_id, orgu_scope,
                   ur_type, valid_from, valid_until, created_at"
    )
    .bind(body.orgtnt_id)
    .bind(body.u_id)
    .bind(body.r_id)
    .bind(body.orgu_id)
    .bind(&body.orgu_scope)
    .bind(body.ur_type.as_deref().unwrap_or("granted"))
    .bind(body.valid_from)
    .bind(body.valid_until)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn delete(pool: &PgPool, ur_id: Uuid) -> Result<(), OrgError> {
    let rows = sqlx::query("DELETE FROM org.ur WHERE ur_id = $1")
        .bind(ur_id)
        .execute(pool)
        .await
        .map_err(OrgError::Database)?
        .rows_affected();

    if rows == 0 {
        return Err(OrgError::NotFound(format!("ur {ur_id}")));
    }
    Ok(())
}
```

- [ ] **Step 3: Update `crates/org/src/repo/mod.rs`**

Add the new modules:

```rust
pub mod dynctx;   // existing — only if present, else skip
pub mod orgt;
pub mod orgtnt;
pub mod orgu;
pub mod role;     // NEW
pub mod ur;       // NEW
pub mod user;     // NEW
pub mod user_role;
```

- [ ] **Step 4: Update `crates/org/src/lib.rs`**

Ensure new models are publicly exported. Add to the existing `pub use` block:

```rust
pub use models::{
    // existing:
    Orgtnt, Orgt, Orgu, OrgUnit,
    // new:
    User, Role, Ur, UOrgu,
    CreateOrgtnt, UpdateOrgtnt,
    CreateOrgt, UpdateOrgt,
    CreateOrgu, UpdateOrgu,
    CreateUser, UpdateUser, AssignUOrgu,
    CreateRole, UpdateRole,
    CreateUr,
};
```

- [ ] **Step 5: Verify compile**

```bash
cargo check -p wf-org 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add crates/org/src/repo/
git commit -m "feat(org): user/role/ur repos + update repo/mod.rs and lib.rs exports"
```

---

## Task 6: Add all CRUD handlers to server org routes

**Files:**
- Modify: `crates/server/src/routes/org.rs`

- [ ] **Step 1: Replace `crates/server/src/routes/org.rs` with full CRUD version**

```rust
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{delete, get, post, put},
    Json, Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use wf_org::{
    models::{
        AssignUOrgu, CreateOrgt, CreateOrgtnt, CreateOrgu,
        CreateRole, CreateUr, CreateUser,
        UpdateOrgt, UpdateOrgtnt, UpdateOrgu,
        UpdateRole, UpdateUser,
    },
    repo,
};
use crate::error::AppError;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        // orgtnt
        .route("/orgtnt",              get(list_orgtnt).post(create_orgtnt))
        .route("/orgtnt/:id",          get(get_orgtnt).put(update_orgtnt))
        // orgt
        .route("/orgt",                post(create_orgt))
        .route("/orgt/:id",            put(update_orgt))
        .route("/orgtnt/:id/orgt",     get(list_orgt_by_tenant))
        // orgu
        .route("/orgt/:id/orgu",       get(list_orgu_by_tree))
        .route("/orgu",                post(create_orgu))
        .route("/orgu/:id",            get(get_orgu).put(update_orgu).delete(delete_orgu))
        .route("/orgu/:id/traverse",   get(traverse_orgu))
        // user
        .route("/user",                get(list_user).post(create_user))
        .route("/user/:id",            put(update_user).delete(delete_user))
        .route("/user/:id/orgu",       post(assign_user_orgu).get(list_user_orgus))
        // role
        .route("/role",                get(list_role).post(create_role))
        .route("/role/:id",            put(update_role).delete(delete_role))
        // ur
        .route("/ur",                  get(list_ur).post(create_ur))
        .route("/ur/:id",              delete(delete_ur))
        .with_state(pool)
}

// ── orgtnt ──────────────────────────────────────────────────────

async fn list_orgtnt(State(pool): State<PgPool>) -> Result<Json<Vec<wf_org::Orgtnt>>, AppError> {
    repo::orgtnt::list(&pool).await.map(Json).map_err(Into::into)
}

async fn get_orgtnt(State(pool): State<PgPool>, Path(id): Path<Uuid>) -> Result<Json<wf_org::Orgtnt>, AppError> {
    repo::orgtnt::get(&pool, id).await.map(Json).map_err(Into::into)
}

async fn create_orgtnt(
    State(pool): State<PgPool>,
    Json(body): Json<CreateOrgtnt>,
) -> Result<(StatusCode, Json<wf_org::Orgtnt>), AppError> {
    let row = repo::orgtnt::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_orgtnt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrgtnt>,
) -> Result<Json<wf_org::Orgtnt>, AppError> {
    repo::orgtnt::update(&pool, id, &body).await.map(Json).map_err(Into::into)
}

// ── orgt ────────────────────────────────────────────────────────

async fn list_orgt_by_tenant(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::Orgt>>, AppError> {
    repo::orgt::list_by_tenant(&pool, orgtnt_id).await.map(Json).map_err(Into::into)
}

async fn create_orgt(
    State(pool): State<PgPool>,
    Json(body): Json<CreateOrgt>,
) -> Result<(StatusCode, Json<wf_org::Orgt>), AppError> {
    let row = repo::orgt::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_orgt(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrgt>,
) -> Result<Json<wf_org::Orgt>, AppError> {
    repo::orgt::update(&pool, id, &body).await.map(Json).map_err(Into::into)
}

// ── orgu ────────────────────────────────────────────────────────

async fn list_orgu_by_tree(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::Orgu>>, AppError> {
    repo::orgu::list_by_tree(&pool, orgt_id).await.map(Json).map_err(Into::into)
}

async fn get_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
) -> Result<Json<wf_org::Orgu>, AppError> {
    repo::orgu::get(&pool, orgu_id).await.map(Json).map_err(Into::into)
}

async fn create_orgu(
    State(pool): State<PgPool>,
    Json(body): Json<CreateOrgu>,
) -> Result<(StatusCode, Json<wf_org::Orgu>), AppError> {
    let row = repo::orgu::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_orgu(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateOrgu>,
) -> Result<Json<wf_org::Orgu>, AppError> {
    repo::orgu::update(&pool, id, &body).await.map(Json).map_err(Into::into)
}

async fn delete_orgu(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    repo::orgu::delete(&pool, id).await.map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct TraverseQuery { expr: String }

async fn traverse_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Query(q): Query<TraverseQuery>,
) -> Result<Json<Vec<wf_org::Orgu>>, AppError> {
    let orgt_id = repo::orgu::get_orgt_id(&pool, orgu_id).await.map_err(AppError::from)?;
    let pipeline = wf_org::traversal::parser::parse(&q.expr)
        .map_err(|e| AppError(e.to_string(), StatusCode::BAD_REQUEST))?;
    let result = wf_org::traversal::executor::execute(&pool, orgu_id, orgt_id, &pipeline)
        .await
        .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    Ok(Json(result))
}

// ── user ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TenantQuery { orgtnt_id: Uuid }

async fn list_user(
    State(pool): State<PgPool>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Vec<wf_org::User>>, AppError> {
    repo::user::list(&pool, q.orgtnt_id).await.map(Json).map_err(Into::into)
}

async fn create_user(
    State(pool): State<PgPool>,
    Json(body): Json<CreateUser>,
) -> Result<(StatusCode, Json<wf_org::User>), AppError> {
    let row = repo::user::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_user(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateUser>,
) -> Result<Json<wf_org::User>, AppError> {
    repo::user::update(&pool, id, &body).await.map(Json).map_err(Into::into)
}

async fn delete_user(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    repo::user::delete(&pool, id).await.map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn assign_user_orgu(
    State(pool): State<PgPool>,
    Path(u_id): Path<Uuid>,
    Json(mut body): Json<AssignUOrgu>,
) -> Result<(StatusCode, Json<wf_org::UOrgu>), AppError> {
    body.u_id = u_id;
    let row = repo::user::assign_orgu(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn list_user_orgus(
    State(pool): State<PgPool>,
    Path(u_id): Path<Uuid>,
) -> Result<Json<Vec<wf_org::UOrgu>>, AppError> {
    repo::user::list_orgus(&pool, u_id).await.map(Json).map_err(Into::into)
}

// ── role ────────────────────────────────────────────────────────

async fn list_role(
    State(pool): State<PgPool>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Vec<wf_org::Role>>, AppError> {
    repo::role::list(&pool, q.orgtnt_id).await.map(Json).map_err(Into::into)
}

async fn create_role(
    State(pool): State<PgPool>,
    Json(body): Json<CreateRole>,
) -> Result<(StatusCode, Json<wf_org::Role>), AppError> {
    let row = repo::role::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn update_role(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateRole>,
) -> Result<Json<wf_org::Role>, AppError> {
    repo::role::update(&pool, id, &body).await.map(Json).map_err(Into::into)
}

async fn delete_role(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    repo::role::delete(&pool, id).await.map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── ur ──────────────────────────────────────────────────────────

async fn list_ur(
    State(pool): State<PgPool>,
    Query(q): Query<TenantQuery>,
) -> Result<Json<Vec<wf_org::Ur>>, AppError> {
    repo::ur::list(&pool, q.orgtnt_id).await.map(Json).map_err(Into::into)
}

async fn create_ur(
    State(pool): State<PgPool>,
    Json(body): Json<CreateUr>,
) -> Result<(StatusCode, Json<wf_org::Ur>), AppError> {
    let row = repo::ur::create(&pool, &body).await.map_err(AppError::from)?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn delete_ur(
    State(pool): State<PgPool>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    repo::ur::delete(&pool, id).await.map_err(AppError::from)?;
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Verify full workspace compile**

```bash
cargo build --workspace 2>&1 | grep "^error"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add crates/server/src/routes/org.rs
git commit -m "feat(server): full org CRUD routes — user/role/ur/orgu/orgt/orgtnt"
```

---

## Task 7: Smoke Test

- [ ] **Step 1: Start server**

```bash
cargo run -p wf-server 2>&1 &
sleep 2
```

Expected: `listening on 0.0.0.0:3000`

- [ ] **Step 2: List orgtnt (existing endpoint, should still work)**

```bash
curl -s http://localhost:3000/org/orgtnt | jq 'length'
```

Expected: a number (0 or more).

- [ ] **Step 3: Create a tenant**

```bash
curl -s -X POST http://localhost:3000/org/orgtnt \
  -H 'Content-Type: application/json' \
  -d '{"name":"Test Corp","code":"test-corp"}' | jq .
```

Expected: `{"orgtnt_id":"...","name":"Test Corp","code":"test-corp","is_active":true,...}`

- [ ] **Step 4: Create a user**

```bash
TENANT_ID=$(curl -s http://localhost:3000/org/orgtnt | jq -r '.[0].orgtnt_id')
curl -s -X POST http://localhost:3000/org/user \
  -H 'Content-Type: application/json' \
  -d "{\"orgtnt_id\":\"$TENANT_ID\",\"username\":\"ahmet\",\"full_name\":\"Ahmet Yılmaz\"}" | jq .
```

Expected: `{"u_id":"...","username":"ahmet","full_name":"Ahmet Yılmaz",...}`

- [ ] **Step 5: Create a role**

```bash
curl -s -X POST http://localhost:3000/org/role \
  -H 'Content-Type: application/json' \
  -d "{\"orgtnt_id\":\"$TENANT_ID\",\"name\":\"mudur\",\"display_name\":\"Müdür\"}" | jq .
```

Expected: `{"r_id":"...","name":"mudur","display_name":"Müdür",...}`

- [ ] **Step 6: Kill server and commit**

```bash
pkill -f wf-server
git add -A
git commit -m "test: org CRUD smoke test passing"
```

---

## Self-Review Checklist

| Spec requirement | Task |
|---|---|
| POST /org/orgtnt, PUT /org/orgtnt/:id | Task 2, Task 6 |
| POST /org/orgt, PUT /org/orgt/:id | Task 2, Task 6 |
| POST /org/orgu, PUT /org/orgu/:id, DELETE /org/orgu/:id | Task 3, Task 6 |
| GET/POST /org/user, PUT/DELETE /org/user/:id | Task 4, Task 6 |
| GET/POST /org/role, PUT/DELETE /org/role/:id | Task 5, Task 6 |
| GET/POST /org/ur, DELETE /org/ur/:id | Task 5, Task 6 |
| User → ORGU assignment | Task 4, Task 6 |
| Existing routes unbroken | Task 6 (full router rewrite preserves all existing routes) |
