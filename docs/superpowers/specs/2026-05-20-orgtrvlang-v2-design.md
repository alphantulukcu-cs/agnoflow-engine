# ORGTRVLANG v2 — Pipeline Traversal Language Design

**Date:** 2026-05-20  
**Status:** Approved  
**Goal:** Replace the fixed 9-pattern ORGTRVLANG with a composable pipeline language that supports arbitrary chaining of traversal steps.

---

## Problem with v1

The current implementation maps each query to one of 9 fixed `TraversalExpr` enum variants. Adding depth or combining steps (e.g., `self.up[bolge].children.children[sube]`, `self.siblings.children[kredi]`) is impossible — the parser has no concept of chaining.

---

## New Grammar

```
expr  = "self" ("." step)*

step  = "parent"
      | "siblings"
      | "siblings" "[" type "]"
      | "children"
      | "children" "[" type "]"
      | "up" "[" type "]"

type  = [a-zA-Z0-9_]+
```

**Rules:**
- Every expression **must** start with `self` — parse error if not.
- `self` alone returns the anchor node itself.
- Steps chain left-to-right; each step's output becomes the next step's input.
- `type` values are free-form strings matching the `orgu_t` column (e.g., `sube`, `bolge`, `root`).

### Example expressions

```
self                                            → anchor node
self.parent                                     → parent of anchor
self.siblings                                   → all siblings
self.siblings[sube]                             → siblings of type sube
self.children                                   → direct children
self.children[il]                               → il-type children
self.up[bolge]                                  → nearest bolge ancestor
self.up[bolge].children                         → children of nearest bolge ancestor
self.up[bolge].children[il]                     → il-type children of nearest bolge ancestor
self.up[bolge].children[il].children[sube]      → sube grandchildren via il (NEW)
self.siblings.children[kredi]                   → kredi children of all siblings (NEW)
self.children[il].children[sube]                → previous v1 pattern, now dynamic
```

---

## AST

```rust
// src/traversal/pipeline.rs

pub struct Pipeline {
    pub steps: Vec<Step>,   // empty = just "self"
}

#[derive(Debug, PartialEq)]
pub enum Step {
    Parent,
    Siblings,
    SiblingsT(String),
    Children,
    ChildrenT(String),
    UpT(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("expression must start with 'self'")]
    MissingSelf,
    #[error("unknown step: {0:?}")]
    UnknownStep(String),
    #[error("missing type in: {0:?}")]
    MissingType(String),
}

pub fn parse(expr: &str) -> Result<Pipeline, ParseError>;
```

---

## Execution Model — Set Semantics

The executor maintains a **current set of ORGU IDs** between steps:

```
execute(anchor_id, orgt_id, pipeline):

    current_ids: Vec<Uuid> = [anchor_id]

    if pipeline.steps is empty:
        return fetch_by_ids(current_ids, orgt_id)   → Vec<Orgu>

    for step in pipeline.steps:
        rows: Vec<Orgu> = execute_step(current_ids, orgt_id, step)
        current_ids = deduplicate(rows.orgu_ids)    ← one SQL per step

    return rows   ← final step's full Orgu results
```

**Key property:** `execute_step` takes `&[Uuid]` and returns `Vec<Orgu>`. This is **one SQL query per step**, regardless of how many nodes are in the current set. Results are deduplicated by `orgu_id` before passing to the next step.

---

## Batch SQL per Step

All queries use `= ANY($1::uuid[])` to handle multi-node inputs in one round trip.
`$1` = `&[Uuid]` (current set), `$2` = `orgt_id`.

### `children` / `children[T]`

```sql
SELECT <COLS> FROM orgu
WHERE parent_orgu_id = ANY($1::uuid[])
  AND orgt_id = $2
  AND is_active = true
```

For `children[T]`: add `AND orgu_t = $3`.

### `parent`

```sql
SELECT DISTINCT <COLS> FROM orgu
WHERE orgu_id IN (
    SELECT parent_orgu_id FROM orgu
    WHERE orgu_id = ANY($1::uuid[])
      AND parent_orgu_id IS NOT NULL
)
AND orgt_id = $2
AND is_active = true
```

### `siblings` / `siblings[T]`

```sql
SELECT DISTINCT <COLS> FROM orgu
WHERE parent_orgu_id IN (
    SELECT parent_orgu_id FROM orgu
    WHERE orgu_id = ANY($1::uuid[])
      AND parent_orgu_id IS NOT NULL
)
AND orgu_id != ALL($1::uuid[])
AND orgt_id = $2
AND is_active = true
```

For `siblings[T]`: add `AND orgu_t = $3`.

### `up[T]` — nearest typed ancestor per node

Uses ltree for ancestor traversal. Returns **one nearest ancestor per anchor node**, deduplicated.

```sql
SELECT DISTINCT anc.orgu_id, anc.orgtnt_id, anc.orgt_id, anc.parent_orgu_id,
       anc.path::text AS path, anc.orgu_t, anc.name, anc.metadata,
       anc.is_active, anc.created_at, anc.updated_at
FROM (
    SELECT DISTINCT ON (anchor.orgu_id) anc.*
    FROM (
        SELECT orgu_id, path FROM orgu WHERE orgu_id = ANY($1::uuid[])
    ) AS anchor
    JOIN orgu anc ON anchor.path <@ anc.path
                  AND anc.orgu_t = $3
                  AND anc.orgt_id = $2
                  AND anc.is_active = true
                  AND anc.path <> anchor.path   -- exclude self
    ORDER BY anchor.orgu_id, nlevel(anc.path) DESC
) anc
```

`DISTINCT ON (anchor.orgu_id)` picks the nearest (deepest) ancestor per anchor. The outer `SELECT DISTINCT` deduplicates across anchors (two anchors may share the same ancestor).

### `self` (bare, no steps)

```sql
SELECT <COLS> FROM orgu
WHERE orgu_id = ANY($1::uuid[])
  AND orgt_id = $2
  AND is_active = true
```

---

## COLS Constant

All queries use the same column selection (ltree cast included):

```rust
const COLS: &str = "orgu_id, orgtnt_id, orgt_id, parent_orgu_id, \
                    path::text AS path, orgu_t, name, metadata, \
                    is_active, created_at, updated_at";
```

---

## HTTP API

Endpoint unchanged:

```
GET /orgu/:id/traverse?expr=<pipeline expression>
```

- `:id` = anchor UUID (the `self` node)
- `expr` must start with `self`

**Errors:**
- `expr` doesn't start with `self` → `400 Bad Request: expression must start with 'self'`
- Unknown step token → `400 Bad Request: unknown step: "foo"`
- Anchor UUID not found → `404 Not Found`

**Examples:**
```
GET /orgu/00000000-0000-0000-0001-000000000004/traverse?expr=self
GET /orgu/00000000-0000-0000-0001-000000000004/traverse?expr=self.parent
GET /orgu/00000000-0000-0000-0001-000000000004/traverse?expr=self.up%5Bbolge%5D.children%5Bil%5D.children%5Bsube%5D
```

---

## Files Changed

| File | Action |
|------|--------|
| `src/traversal/parser.rs` | **Deleted** |
| `src/traversal/pipeline.rs` | **Created** — `Pipeline`, `Step`, `ParseError`, `parse()` |
| `src/traversal/executor.rs` | **Replaced** — new set-based `execute()` + per-step functions |
| `src/traversal/mod.rs` | Updated: `parser` → `pipeline` |
| `src/handlers/traverse.rs` | Updated: `parser::parse` → `pipeline::parse` |

**No changes to:**
- `init.sql` — old 9 SQL functions remain (unused by new code)
- `src/models.rs`, `src/handlers/orgu.rs`, etc.

---

## Tests

**Unit tests (`pipeline.rs`):**
- `parse("self")` → empty steps
- `parse("self.children")` → `[Children]`
- `parse("self.children[il]")` → `[ChildrenT("il")]`
- `parse("self.up[bolge].children[il].children[sube]")` → `[UpT("bolge"), ChildrenT("il"), ChildrenT("sube")]`
- `parse("self.siblings.children[kredi]")` → `[Siblings, ChildrenT("kredi")]`
- `parse("children")` → `ParseError::MissingSelf`
- `parse("self.garbage")` → `ParseError::UnknownStep("garbage")`

**Integration tests (`executor.rs`):**

All use QNB test data (anchor = Çankaya Şubesi `...0004`, ORGT = `...0010`):

| Expression | Anchor | Expected |
|-----------|--------|----------|
| `self` | Çankaya | [Çankaya Şubesi] |
| `self.parent` | Çankaya | [Ankara İl Md.] |
| `self.siblings` | Çankaya | [Keçiören, Kredi Ankara] |
| `self.siblings[sube]` | Çankaya | [Keçiören] |
| `self.children` | Ankara İl | [Çankaya, Keçiören, Kredi] |
| `self.children[sube]` | Ankara İl | [Çankaya, Keçiören] |
| `self.up[bolge]` | Çankaya | [İç Anadolu] |
| `self.up[bolge].children` | Çankaya | [Ankara İl, Eskişehir İl] |
| `self.up[bolge].children[il]` | Çankaya | [Ankara İl, Eskişehir İl] |
| `self.up[bolge].children[il].children[sube]` | Çankaya | [Çankaya, Keçiören, Odunpazarı, Tepebaşı] |
| `self.siblings.children[sube]` | Ankara İl (0003) | [Odunpazarı, Tepebaşı] (Eskişehir'in sube çocukları) |

---

## What v1 Expressed vs v2

| v1 expression | v2 equivalent |
|--------------|---------------|
| `self` | `self` |
| `parent` | `self.parent` |
| `siblings` | `self.siblings` |
| `siblings[T]` | `self.siblings[T]` |
| `children` | `self.children` |
| `children[T]` | `self.children[T]` |
| `up[T]` | `self.up[T]` |
| `up[T].children` | `self.up[T].children` |
| `up[T].children[T2]` | `self.up[T].children[T2]` |
| `children[T].children[T2]` | `self.children[T].children[T2]` |
| (impossible) | `self.up[T].children[T2].children[T3]` |
| (impossible) | `self.siblings.children[T]` |
