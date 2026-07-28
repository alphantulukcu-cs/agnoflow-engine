# Multi Org Tree Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a tenant have more than one org tree (`org.orgt`), mark one as default, and
switch between them from the Organizations page.

**Architecture:** Backend: add `is_default` to `org.orgt` (DB-enforced single-default-per-
tenant via a partial unique index), plus create/rename/set-default repo functions and
routes mirroring the existing `org.r` (role)/`org.orgu_type_def` CRUD pattern. Frontend:
`OrgExplorer` currently loads exactly one tree per tenant and never lists the others —
this adds a tree list fetch, a switcher `<select>` + management modal in the page header
(next to the existing "+ Yeni Ekle"/"Workflow" buttons and orgu/kullanıcı/rol badges),
and updates `App.tsx`'s bootstrap to pick the tenant's default tree instead of the first
one returned.

**Tech Stack:** Rust (axum, sqlx), Postgres, React + TypeScript, Zustand.

**Spec:** `docs/superpowers/specs/2026-07-28-multi-org-tree-design.md`

---

## Backend (agnoflow-engine)

### Task 1: Migration — `org.orgt.is_default` + backfill + uniqueness

**Files:**
- Create: `migrations/org/20260728000002_orgt_default.sql`

- [ ] **Step 1: Write the migration**

```sql
-- migrations/org/20260728000002_orgt_default.sql
-- Tenant başına çoklu org ağacı: "varsayılan" salt UI/bootstrap kolaylığıdır — motor
-- (traversal/yetkilendirme) her zaman anchor node'un kendi ağacını çözer, is_default'a
-- hiç bakmaz.

ALTER TABLE org.orgt ADD COLUMN is_default boolean NOT NULL DEFAULT false;

-- Geriye dönük uyumluluk: her tenant'ın bugün var olan (en eski) aktif ağacı varsayılan olsun.
UPDATE org.orgt o
SET is_default = true
WHERE o.orgt_id = (
    SELECT o2.orgt_id FROM org.orgt o2
    WHERE o2.orgtnt_id = o.orgtnt_id AND o2.is_active = true
    ORDER BY o2.created_at ASC LIMIT 1
);

-- Tenant başına en fazla bir varsayılan — DB seviyesinde garanti.
CREATE UNIQUE INDEX orgt_one_default_per_tenant
    ON org.orgt (orgtnt_id) WHERE is_default = true;
```

- [ ] **Step 2: Apply the migration against the dev database**

This repo applies migrations manually via `psql` (no `sqlx migrate` tracking table).
If `psql`/Docker aren't available locally, use the throwaway `sqlx::raw_sql` example
binary approach (add `crates/server/examples/apply_sql_file.rs` temporarily — connects
via `sqlx::PgPool` + `dotenvy::dotenv()`, runs `sqlx::raw_sql(&file_contents)`; delete
the file when done, never commit it). Otherwise:

Run: `psql "$DATABASE_URL" -f migrations/org/20260728000002_orgt_default.sql`
Expected: `ALTER TABLE`, `UPDATE 1` (per existing tenant), `CREATE INDEX`, no errors.

- [ ] **Step 3: Verify**

Run: `psql "$DATABASE_URL" -c "SELECT orgtnt_id, orgt_id, name, is_default FROM org.orgt ORDER BY orgtnt_id;"`
(or the `--query` mode of the throwaway example binary)
Expected: every existing tenant's tree shows `is_default = true`.

- [ ] **Step 4: Commit**

```bash
git add migrations/org/20260728000002_orgt_default.sql
git commit -m "feat(org): org.orgt.is_default — tenant başına varsayılan ağaç"
```

---

### Task 2: Model — `Orgt.is_default`

**Files:**
- Modify: `crates/org/src/models.rs`

- [ ] **Step 1: Add the field**

```rust
pub struct Orgt {
    pub orgt_id: Uuid,
    pub orgtnt_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p wf-org`
Expected: errors at every `Orgt { ... }` struct-literal construction site missing
`is_default` — there should be none, since all existing code builds `Orgt` via
`query_as::<_, Orgt>` (column mapping), not struct literals. Confirm with:
Run: `grep -rn "Orgt {" crates/ --include=*.rs`
Expected: no matches (or only this struct's own definition).

- [ ] **Step 3: Commit**

```bash
git add crates/org/src/models.rs
git commit -m "feat(org): Orgt modeline is_default eklendi"
```

---

### Task 3: Repo — `orgt::create`, `orgt::update`, `orgt::set_default`

**Files:**
- Modify: `crates/org/src/repo/orgt.rs`

- [ ] **Step 1: Read the current file to confirm the exact `list_by_tenant`/`get_orgtnt_id` column list**

The existing `list_by_tenant` selects: `orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at`.
Every new query below must also select `is_default` in that same column order convention.

- [ ] **Step 2: Add the three functions**

Append to `crates/org/src/repo/orgt.rs` (after `get_orgtnt_id`):

```rust
const SEL: &str = "orgt_id, orgtnt_id, name, description, is_active, is_default, created_at, updated_at";

/// Tenant'ın hiç aktif ağacı yoksa yeni ağaç otomatik varsayılan olur — "bir tanesi
/// default olacak" hiçbir zaman ihlal edilmez.
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Orgt, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("ağaç adı boş olamaz".into()));
    }
    let has_active: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM org.orgt WHERE orgtnt_id = $1 AND is_active = true)",
    )
    .bind(orgtnt_id)
    .fetch_one(pool)
    .await?;

    sqlx::query_as::<_, Orgt>(&format!(
        "INSERT INTO org.orgt (orgtnt_id, name, description, is_default)
         VALUES ($1, $2, $3, $4)
         RETURNING {SEL}"
    ))
    .bind(orgtnt_id)
    .bind(name)
    .bind(description)
    .bind(!has_active)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update(
    pool: &PgPool,
    orgt_id: Uuid,
    name: &str,
    description: Option<&str>,
) -> Result<Orgt, OrgError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(OrgError::BadRequest("ağaç adı boş olamaz".into()));
    }
    sqlx::query_as::<_, Orgt>(&format!(
        "UPDATE org.orgt SET name = $2, description = $3, updated_at = now()
         WHERE orgt_id = $1
         RETURNING {SEL}"
    ))
    .bind(orgt_id)
    .bind(name)
    .bind(description)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgt {orgt_id}")))
}

/// Transaction: önce tenant'ın mevcut varsayılanı false'a düşer, sonra hedef true olur —
/// partial unique index (`orgt_one_default_per_tenant`) hiçbir ara adımda ihlal edilmez.
pub async fn set_default(pool: &PgPool, orgtnt_id: Uuid, orgt_id: Uuid) -> Result<Orgt, OrgError> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE org.orgt SET is_default = false WHERE orgtnt_id = $1 AND is_default = true")
        .bind(orgtnt_id)
        .execute(&mut *tx)
        .await?;

    let result = sqlx::query("UPDATE org.orgt SET is_default = true WHERE orgt_id = $1 AND orgtnt_id = $2")
        .bind(orgt_id)
        .bind(orgtnt_id)
        .execute(&mut *tx)
        .await?;
    if result.rows_affected() == 0 {
        return Err(OrgError::NotFound(format!("orgt {orgt_id} bu tenant'ta yok")));
    }

    tx.commit().await?;

    sqlx::query_as::<_, Orgt>(&format!("SELECT {SEL} FROM org.orgt WHERE orgt_id = $1"))
        .bind(orgt_id)
        .fetch_one(pool)
        .await
        .map_err(OrgError::Database)
}
```

Also update the existing `list_by_tenant` function's SQL string to select `is_default`
too (it currently lists columns explicitly without it):

Find: `"SELECT orgt_id, orgtnt_id, name, description, is_active, created_at, updated_at`
Replace with: `"SELECT {SEL}` — i.e. reuse the new `SEL` constant instead of the inline
column list, so both queries can never drift apart. Since `SEL` is defined above
`list_by_tenant` in the file, move the `const SEL` line to the top of the file (right
after the imports) so both functions can reference it.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p wf-org`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add crates/org/src/repo/orgt.rs
git commit -m "feat(org): orgt create/update/set_default repo fonksiyonları"
```

---

### Task 4: Routes — orgt create/update/set-default

**Files:**
- Modify: `crates/server/src/routes/org.rs`

- [ ] **Step 1: Add request body type and handlers**

Add near `list_orgt_by_tenant` (after it):

```rust
#[derive(Deserialize, ToSchema)]
struct OrgtBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[utoipa::path(post, path = "/orgtnt/{id}/orgt", tag = "org",
    params(("id" = Uuid, Path, description = "Tenant id")), request_body = OrgtBody,
    responses((status = 200, description = "Oluşturulan org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn create_orgt(
    State(pool): State<PgPool>,
    Path(orgtnt_id): Path<Uuid>,
    Json(body): Json<OrgtBody>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    repo::orgt::create(&pool, orgtnt_id, &body.name, body.description.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(patch, path = "/orgt/{id}", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id")), request_body = OrgtBody,
    responses((status = 200, description = "Güncellenen org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn update_orgt(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
    Json(body): Json<OrgtBody>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    repo::orgt::update(&pool, orgt_id, &body.name, body.description.as_deref())
        .await
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(post, path = "/orgt/{id}/set-default", tag = "org",
    params(("id" = Uuid, Path, description = "Org ağacı id")),
    responses((status = 200, description = "Varsayılan yapılan org ağacı", body = serde_json::Value)),
    security(("x_admin_key" = [])))]
async fn set_default_orgt(
    State(pool): State<PgPool>,
    Path(orgt_id): Path<Uuid>,
) -> Result<Json<wf_org::models::Orgt>, AppError> {
    let orgtnt_id = repo::orgt::get_orgtnt_id(&pool, orgt_id)
        .await
        .map_err(AppError::from)?;
    repo::orgt::set_default(&pool, orgtnt_id, orgt_id)
        .await
        .map(Json)
        .map_err(Into::into)
}
```

- [ ] **Step 2: Register the routes in `router()`**

Find:
```rust
        .routes(routes!(list_orgt_by_tenant))
```
Replace with (same path `/orgtnt/{id}/orgt`, must be grouped together per axum's
same-path rule — same gotcha documented in the orgu-crud plan):
```rust
        .routes(routes!(list_orgt_by_tenant, create_orgt))
        .routes(routes!(update_orgt))
        .routes(routes!(set_default_orgt))
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p wf-server`
Expected: no errors.

- [ ] **Step 4: Run the full workspace test suite**

Run: `cargo test --workspace`
Expected: all existing tests still pass (no behavior change to existing endpoints).

- [ ] **Step 5: Manual smoke test against a running local server**

Use an isolated port (e.g. `PORT=3001`) so you don't collide with any server the user
already has running — check with `ss -ltnp | grep 3000` first and never kill a
pre-existing process that isn't yours.

```bash
ORGTNT_ID=$(curl -s http://localhost:3001/org/orgtnt | python3 -c "import json,sys; print(json.load(sys.stdin)[0]['orgtnt_id'])")

# Existing tree should show is_default=true after the migration.
curl -s "http://localhost:3001/org/orgtnt/$ORGTNT_ID/orgt"

# Create a second tree — should come back with is_default=false.
curl -s -X POST "http://localhost:3001/org/orgtnt/$ORGTNT_ID/orgt" \
  -H 'content-type: application/json' \
  -d '{"name": "Merkezi Yönetim", "description": "Merkez birimleri"}'

# Rename it.
curl -s -X PATCH "http://localhost:3001/org/orgt/<new orgt_id>" \
  -H 'content-type: application/json' \
  -d '{"name": "Merkezi Yönetim Ağacı", "description": "Merkez birimleri"}'

# Make it the default — the OLD default should flip to false (verify with the list call again).
curl -s -X POST "http://localhost:3001/org/orgt/<new orgt_id>/set-default"
curl -s "http://localhost:3001/org/orgtnt/$ORGTNT_ID/orgt"
```

Expected: after `set-default`, exactly one tree in the list has `is_default: true` (the
new one), and it's the one just created/renamed.

- [ ] **Step 6: Commit**

```bash
git add crates/server/src/routes/org.rs
git commit -m "feat(server): orgt create/update/set-default endpoint'leri"
```

---

## Frontend (agnoflow-frontend)

### Task 5: API client — tree CRUD + `is_default`

**Files:**
- Modify: `src/api/engineApi.ts`

- [ ] **Step 1: Add `is_default` to `OrgTree` and the three new functions**

```typescript
export interface OrgTree {
  orgt_id: string;
  orgtnt_id: string;
  name: string;
  description?: string | null;
  is_active: boolean;
  is_default: boolean;
}
```

Add after `listTrees`:

```typescript
export function createOrgTree(
  baseUrl: string,
  orgtntId: string,
  adminKey: string,
  body: { name: string; description?: string },
): Promise<OrgTree> {
  return request(baseUrl, `/org/orgtnt/${orgtntId}/orgt`, {
    method: 'POST',
    body: JSON.stringify(body),
    headers: adminHeaders(adminKey),
  });
}

export function updateOrgTree(
  baseUrl: string,
  orgtId: string,
  adminKey: string,
  body: { name: string; description?: string },
): Promise<OrgTree> {
  return request(baseUrl, `/org/orgt/${orgtId}`, {
    method: 'PATCH',
    body: JSON.stringify(body),
    headers: adminHeaders(adminKey),
  });
}

export function setDefaultOrgTree(baseUrl: string, orgtId: string, adminKey: string): Promise<OrgTree> {
  return request(baseUrl, `/org/orgt/${orgtId}/set-default`, {
    method: 'POST',
    headers: adminHeaders(adminKey),
  });
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/api/engineApi.ts
git commit -m "feat(api): org ağacı create/update/set-default client fonksiyonları + is_default alanı"
```

---

### Task 6: `App.tsx` bootstrap — pick the default tree

**Files:**
- Modify: `src/components/App.tsx`

- [ ] **Step 1: Update the tree-selection line**

Find (in the bootstrap effect):
```typescript
        const tree = trees.find((item) => item.orgt_id === stored.orgtId) ?? trees[0];
```
Replace with:
```typescript
        const tree = trees.find((item) => item.orgt_id === stored.orgtId)
          ?? trees.find((item) => item.is_default)
          ?? trees[0];
```

(Priority: last-used tree from localStorage, then the tenant's default, then whatever
comes first — matches the approved spec exactly.)

- [ ] **Step 2: Verify it compiles**

Run: `npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/App.tsx
git commit -m "feat(app): bootstrap artık tenant'ın varsayılan org ağacını seçiyor"
```

---

### Task 7: `OrgTreeManagerModal` component

**Files:**
- Create: `src/components/OrgTreeManagerModal.tsx`

This mirrors `src/components/OrguTypeManagerModal.tsx` almost line-for-line (same
`ModalShell` usage, same create-form-at-top + list-with-inline-edit pattern) — the
differences: fields are `name`/`description` instead of `key`/`display_name`, and each
row gets either a "Varsayılan yap" button (non-default trees) or a static "Varsayılan"
badge (the default tree) instead of a delete button (tree deactivation is out of scope
per the approved spec).

- [ ] **Step 1: Write the component**

```tsx
// src/components/OrgTreeManagerModal.tsx
import { useEffect, useState } from 'react';
import { Network } from 'lucide-react';
import {
  createOrgTree, listTrees, updateOrgTree, setDefaultOrgTree,
  type OrgTree,
} from '../api/engineApi';
import { ModalShell } from './shared/ModalShell';

interface Props {
  isOpen: boolean;
  onClose: () => void;
  onChanged: () => void;
  baseUrl: string;
  orgtntId: string;
  adminKey: string;
}

const inputStyle: React.CSSProperties = {
  minWidth: 0,
  background: 'var(--app-control)',
  color: 'var(--app-text)',
  border: '1px solid var(--app-border)',
  borderRadius: 7,
  padding: '8px 10px',
  fontSize: 12.5,
  fontFamily: 'var(--app-font-sans)',
};

export default function OrgTreeManagerModal({ isOpen, onClose, onChanged, baseUrl, orgtntId, adminKey }: Props) {
  const [trees, setTrees] = useState<OrgTree[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState('');
  const [newDescription, setNewDescription] = useState('');
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  const [editDescription, setEditDescription] = useState('');
  const [busy, setBusy] = useState(false);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      setTrees(await listTrees(baseUrl, orgtntId));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (isOpen) void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  const handleCreate = async () => {
    if (!newName.trim()) {
      setError('Ağaç adı girin.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await createOrgTree(baseUrl, orgtntId, adminKey, {
        name: newName.trim(),
        ...(newDescription.trim() ? { description: newDescription.trim() } : {}),
      });
      setNewName('');
      setNewDescription('');
      await refresh();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const startEdit = (tree: OrgTree) => {
    setEditingId(tree.orgt_id);
    setEditName(tree.name);
    setEditDescription(tree.description ?? '');
  };

  const handleUpdate = async () => {
    if (!editingId || !editName.trim()) return;
    setBusy(true);
    setError(null);
    try {
      await updateOrgTree(baseUrl, editingId, adminKey, {
        name: editName.trim(),
        ...(editDescription.trim() ? { description: editDescription.trim() } : {}),
      });
      setEditingId(null);
      await refresh();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleSetDefault = async (tree: OrgTree) => {
    setBusy(true);
    setError(null);
    try {
      await setDefaultOrgTree(baseUrl, tree.orgt_id, adminKey);
      await refresh();
      onChanged();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <ModalShell
      isOpen={isOpen}
      onClose={onClose}
      title="Ağaçlar"
      subtitle="Tenant'ın org ağaçları — biri varsayılan olarak işaretlenir"
      icon={<Network size={15} />}
      width={480}
    >
      {error && (
        <div style={{ marginBottom: 12, background: 'rgba(220,38,38,0.10)', border: '1px solid rgba(220,38,38,0.28)', color: 'var(--app-danger)', borderRadius: 8, padding: '9px 11px', fontSize: 12 }}>
          {error}
        </div>
      )}

      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr auto', gap: 8, marginBottom: 16 }}>
        <input style={inputStyle} placeholder="Ağaç adı" value={newName} onChange={(e) => setNewName(e.target.value)} disabled={busy} />
        <input style={inputStyle} placeholder="Açıklama (opsiyonel)" value={newDescription} onChange={(e) => setNewDescription(e.target.value)} disabled={busy} />
        <button onClick={() => void handleCreate()} disabled={busy} style={{ background: 'var(--app-accent)', color: 'var(--app-accent-text)', border: 'none', borderRadius: 7, padding: '0 14px', fontSize: 12, fontWeight: 800, cursor: 'pointer' }}>
          Ekle
        </button>
      </div>

      {loading ? (
        <div style={{ color: 'var(--app-muted)', fontSize: 12.5 }}>Yükleniyor...</div>
      ) : trees.length === 0 ? (
        <div style={{ color: 'var(--app-muted)', fontSize: 12.5 }}>Henüz ağaç yok.</div>
      ) : trees.map((tree) => (
        <div key={tree.orgt_id} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 0', borderBottom: '1px solid var(--app-border-soft)' }}>
          {editingId === tree.orgt_id ? (
            <>
              <input style={{ ...inputStyle, flex: 1 }} value={editName} onChange={(e) => setEditName(e.target.value)} disabled={busy} />
              <input style={{ ...inputStyle, flex: 1 }} value={editDescription} onChange={(e) => setEditDescription(e.target.value)} disabled={busy} />
              <button onClick={() => void handleUpdate()} disabled={busy} style={{ fontSize: 11.5, fontWeight: 700, cursor: 'pointer', border: '1px solid var(--app-border)', borderRadius: 6, padding: '6px 10px', background: 'transparent', color: 'var(--app-accent)' }}>Kaydet</button>
            </>
          ) : (
            <>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13, fontWeight: 700, color: 'var(--app-text-strong)' }}>{tree.name}</div>
                {tree.description && <div style={{ fontSize: 11, color: 'var(--app-muted)' }}>{tree.description}</div>}
              </div>
              {tree.is_default ? (
                <span style={{ fontSize: 10.5, fontWeight: 700, padding: '3px 9px', borderRadius: 999, background: 'color-mix(in srgb, var(--app-accent) 12%, transparent)', color: 'var(--app-accent)', border: '1px solid color-mix(in srgb, var(--app-accent) 28%, transparent)' }}>
                  Varsayılan
                </span>
              ) : (
                <button onClick={() => void handleSetDefault(tree)} disabled={busy} style={{ fontSize: 11.5, fontWeight: 700, cursor: 'pointer', border: '1px solid var(--app-border)', borderRadius: 6, padding: '6px 10px', background: 'transparent', color: 'var(--app-muted)' }}>Varsayılan yap</button>
              )}
              <button onClick={() => startEdit(tree)} disabled={busy} style={{ fontSize: 11.5, fontWeight: 700, cursor: 'pointer', border: '1px solid var(--app-border)', borderRadius: 6, padding: '6px 10px', background: 'transparent', color: 'var(--app-muted)' }}>Düzenle</button>
            </>
          )}
        </div>
      ))}
    </ModalShell>
  );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/OrgTreeManagerModal.tsx
git commit -m "feat(ui): OrgTreeManagerModal — org ağaçları listesi/oluşturma/varsayılan"
```

---

### Task 8: Wire the switcher into `OrgExplorer.tsx`

**Files:**
- Modify: `src/OrgExplorer.tsx`

- [ ] **Step 1: Import the new modal + API functions + store selector**

```typescript
import OrgTreeManagerModal from './components/OrgTreeManagerModal';
```
Add `listTrees` and `type OrgTree` to the existing `./api/engineApi` import line.

- [ ] **Step 2: Add `trees` state and fold the fetch into `loadOrgData`**

Add near the other state declarations (`const [roles, setRoles] = useState<OrgRole[]>([]);`):
```typescript
  const [trees, setTrees] = useState<OrgTree[]>([]);
  const [treeManagerOpen, setTreeManagerOpen] = useState(false);
```

Add the store selector next to `const orgtId = useOrgDataStore((s) => s.orgtId);`:
```typescript
  const setOrgDataConfig = useOrgDataStore((s) => s.setConfig);
```

Update `loadOrgData` to also fetch trees:
```typescript
  const loadOrgData = useCallback(async () => {
    if (!tenantId) {
      setUsers([]);
      setRoles([]);
      setActors([]);
      setTrees([]);
      return;
    }
    try {
      const [usersData, rolesData, actorsData, treesData] = await Promise.all([
        listUsers(baseUrl, tenantId),
        listRoles(baseUrl, tenantId),
        listActors(baseUrl, tenantId),
        listTrees(baseUrl, tenantId),
      ]);
      setUsers(usersData);
      setRoles(rolesData);
      setActors(actorsData);
      setTrees(treesData);
    } catch (error) {
      console.error('Org verisi yüklenemedi:', error);
      setUsers([]);
      setRoles([]);
      setActors([]);
      setTrees([]);
    }
  }, [baseUrl, tenantId]);
```

- [ ] **Step 3: Add the switch-tree handler**

Add near `handleNodeClick`:
```typescript
  // Ağaç switcher — org-data store'un config'ini yeni ağaca çevirir ve o ağacın
  // birimlerini yükler. Oturum içi bir seçimdir; localStorage'a yazılmaz (bir sonraki
  // sayfa yüklemesinde bootstrap yine tenant'ın varsayılan ağacını seçer).
  const handleSwitchTree = useCallback((nextOrgtId: string) => {
    if (!tenantId || nextOrgtId === orgtId) return;
    setOrgDataConfig(baseUrl, tenantId, nextOrgtId);
    void refreshOrgusTree();
  }, [baseUrl, tenantId, orgtId, setOrgDataConfig, refreshOrgusTree]);
```

- [ ] **Step 4: Render the switcher + "Ağaçlar" button in the header**

Find the header block containing the "+ Yeni Ekle" button (inside the
`<div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>` that holds the
orgu/kullanıcı/rol badges and buttons). Add the switcher `<select>` and "Ağaçlar" button
right before the existing "+ Yeni Ekle" button:

```tsx
            <select
              value={orgtId}
              onChange={(event) => handleSwitchTree(event.target.value)}
              style={{ background: 'var(--app-bg)', color: 'var(--app-text)', border: '1px solid var(--app-border-strong)', borderRadius: 7, padding: '7px 10px', fontSize: 12, fontFamily: 'var(--app-font-sans)', cursor: 'pointer' }}
            >
              {trees.map((tree) => (
                <option key={tree.orgt_id} value={tree.orgt_id}>
                  {tree.name}{tree.is_default ? ' (varsayılan)' : ''}
                </option>
              ))}
            </select>
            <button
              onClick={() => setTreeManagerOpen(true)}
              style={{ background: 'var(--app-control)', color: 'var(--app-text-soft)', border: '1px solid var(--app-border)', borderRadius: 7, padding: '8px 12px', fontSize: 12, fontWeight: 700, cursor: 'pointer', fontFamily: 'var(--app-font-sans)' }}
            >
              Ağaçlar
            </button>
```

- [ ] **Step 5: Render the modal**

Add right before the component's closing `</div>` (alongside where other modals/panels
would be rendered — this component currently has no other top-level modals, so add it
as the last child of the outermost `<div>`, after `</main>`):

```tsx
      <OrgTreeManagerModal
        isOpen={treeManagerOpen}
        onClose={() => setTreeManagerOpen(false)}
        onChanged={loadOrgData}
        baseUrl={baseUrl}
        orgtntId={tenantId ?? ''}
        adminKey={effectiveAdminKey}
      />
```

- [ ] **Step 6: Verify it compiles**

Run: `npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 7: Run the full frontend test suite**

Run: `npx vitest run`
Expected: all existing tests pass.

- [ ] **Step 8: Commit**

```bash
git add src/OrgExplorer.tsx
git commit -m "feat(ui): Organizations sayfasına ağaç switcher'ı ve Ağaçlar yönetim modalı"
```

---

### Task 9: Manual end-to-end verification

**Files:** none (verification only)

- [ ] **Step 1: Start an isolated backend + frontend pair**

Use non-default ports (e.g. backend `PORT=3001` with `CORS_ORIGINS` including whatever
port the frontend dev server picks) so you never touch a server/dev-server the user
already has running. Check `ss -ltnp` first; never kill a pre-existing process you
didn't start yourself.

- [ ] **Step 2: Exercise the golden path**

1. Log in (or use the existing headless-chromium + real backend approach if no
   credentials are available — screenshot what you can, be explicit about what you
   couldn't verify due to missing login credentials).
2. Open Organizations → confirm the tree switcher shows the tenant's existing tree
   marked "(varsayılan)".
3. Click "Ağaçlar" → create a new tree ("Merkezi Yönetim") → confirm it appears in the
   modal's list and in the header switcher's dropdown.
4. Switch to the new tree in the header dropdown → confirm the Org Ağacı tab now shows
   an empty tree (0 birim) — "+ Yeni Ekle" should let you create a root unit in *this*
   tree.
5. Switch back to the original tree → confirm its units reappear.
6. In "Ağaçlar", click "Varsayılan yap" on the new tree → confirm the badge moves and
   the original tree's row now shows a "Varsayılan yap" button instead of the badge.
7. Rename a tree via "Düzenle" → confirm the switcher dropdown's label updates.

- [ ] **Step 3: Clean up**

Stop only the test server/dev-server instances you started (verify by PID/port before
killing anything).

---

## Self-review notes

- **Spec coverage:** §3 (data model) → Task 1. §4 (backend) → Tasks 2–4. §5 (frontend) →
  Tasks 5–8. §6 (out of scope: deactivation, WFD↔tree binding) → deliberately not
  implemented anywhere in this plan. §7 (test plan) → Task 4 Steps 4–5 (workspace tests +
  manual smoke test), Task 9 (frontend manual verification).
- **Type consistency:** `OrgtBody` (backend request body) fields (`name`, `description`)
  match `createOrgTree`/`updateOrgTree`'s TypeScript body shape exactly. `is_default`
  naming is consistent across the Rust `Orgt` struct, the `OrgTree` TypeScript interface,
  and every UI surface (switcher option suffix, modal badge/button).
- **No DB test harness exists for `crates/org`** (confirmed during the earlier orgu-crud
  work) — Task 4 Step 5's manual curl smoke test is the real verification for the
  transactional `set_default` logic, matching how this codebase is actually verified.
