# ORGTRVLANG v2 — Pipeline Language Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fixed 9-pattern ORGTRVLANG with a composable pipeline language (`self.up[bolge].children[il].children[sube]`) that supports arbitrary step chaining.

**Architecture:** New `pipeline.rs` replaces `parser.rs` with a proper AST (`Pipeline { steps: Vec<Step> }`). The executor is rewritten to run each step as a single batch SQL query against all current IDs (using `ANY($1::uuid[])`), iterating in Rust with deduplication between steps. The HTTP endpoint is unchanged.

**Tech Stack:** Rust, Axum 0.7, SQLx 0.7 (postgres + ltree), existing QNB test data in PostgreSQL.

---

## File Map

| File | Action |
|------|--------|
| `src/traversal/pipeline.rs` | **Create** — `Pipeline`, `Step`, `ParseError`, `parse()`, unit tests |
| `src/traversal/executor.rs` | **Replace entirely** — set-based `execute()`, `execute_step()`, integration tests |
| `src/traversal/mod.rs` | **Modify** — swap `parser` → `pipeline` |
| `src/handlers/traverse.rs` | **Modify** — `parser::parse` → `pipeline::parse` |
| `src/traversal/parser.rs` | **Delete** |

All work happens in `/home/alphan/Desktop/workflow-engine/org-api/`.

---

## Task 1: Pipeline Parser (TDD)

**Files:**
- Create: `src/traversal/pipeline.rs`
- Modify: `src/traversal/mod.rs`

- [ ] **Step 1: Write the stub + failing tests in `src/traversal/pipeline.rs`**

Write the full file — types, stub `parse()` that always returns Err, and the full test suite:

```rust
#[derive(Debug)]
pub struct Pipeline {
    pub steps: Vec<Step>,
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
}

pub fn parse(_expr: &str) -> Result<Pipeline, ParseError> {
    Err(ParseError::MissingSelf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(expr: &str) -> Vec<Step> {
        parse(expr).unwrap().steps
    }

    #[test]
    fn test_bare_self() {
        assert_eq!(steps("self"), vec![]);
    }

    #[test]
    fn test_parent() {
        assert_eq!(steps("self.parent"), vec![Step::Parent]);
    }

    #[test]
    fn test_siblings() {
        assert_eq!(steps("self.siblings"), vec![Step::Siblings]);
    }

    #[test]
    fn test_siblings_t() {
        assert_eq!(steps("self.siblings[sube]"), vec![Step::SiblingsT("sube".into())]);
    }

    #[test]
    fn test_children() {
        assert_eq!(steps("self.children"), vec![Step::Children]);
    }

    #[test]
    fn test_children_t() {
        assert_eq!(steps("self.children[il]"), vec![Step::ChildrenT("il".into())]);
    }

    #[test]
    fn test_up_t() {
        assert_eq!(steps("self.up[bolge]"), vec![Step::UpT("bolge".into())]);
    }

    #[test]
    fn test_two_step_chain() {
        assert_eq!(
            steps("self.up[bolge].children[il]"),
            vec![Step::UpT("bolge".into()), Step::ChildrenT("il".into())]
        );
    }

    #[test]
    fn test_three_step_chain() {
        assert_eq!(
            steps("self.up[bolge].children[il].children[sube]"),
            vec![
                Step::UpT("bolge".into()),
                Step::ChildrenT("il".into()),
                Step::ChildrenT("sube".into()),
            ]
        );
    }

    #[test]
    fn test_siblings_then_children() {
        assert_eq!(
            steps("self.siblings.children[kredi]"),
            vec![Step::Siblings, Step::ChildrenT("kredi".into())]
        );
    }

    #[test]
    fn test_missing_self() {
        assert!(matches!(parse("children"), Err(ParseError::MissingSelf)));
        assert!(matches!(parse("up[bolge].children"), Err(ParseError::MissingSelf)));
    }

    #[test]
    fn test_unknown_step() {
        assert!(matches!(parse("self.garbage"), Err(ParseError::UnknownStep(_))));
    }

    #[test]
    fn test_whitespace_trimmed() {
        assert_eq!(steps("  self  "), vec![]);
    }
}
```

- [ ] **Step 2: Run tests — expect failures**

```bash
cd /home/alphan/Desktop/workflow-engine/org-api
cargo test traversal::pipeline
```

Expected: most tests FAIL (stub always returns `Err(MissingSelf)`). `test_missing_self` passes.

- [ ] **Step 3: Implement `parse()` in `src/traversal/pipeline.rs`**

Replace the stub `parse()` with the real implementation (keep all other code unchanged):

```rust
pub fn parse(expr: &str) -> Result<Pipeline, ParseError> {
    let expr = expr.trim();

    let rest = expr
        .strip_prefix("self")
        .ok_or(ParseError::MissingSelf)?;

    if rest.is_empty() {
        return Ok(Pipeline { steps: vec![] });
    }

    if !rest.starts_with('.') {
        return Err(ParseError::MissingSelf);
    }

    let tokens = split_tokens(&rest[1..]); // skip the leading '.'
    let steps = tokens
        .into_iter()
        .map(parse_step)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Pipeline { steps })
}

fn split_tokens(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => {
                tokens.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    tokens.push(&s[start..]);
    tokens
}

fn parse_step(token: &str) -> Result<Step, ParseError> {
    match token {
        "parent"   => return Ok(Step::Parent),
        "siblings" => return Ok(Step::Siblings),
        "children" => return Ok(Step::Children),
        _          => {}
    }

    if let Some(rest) = token.strip_prefix("siblings[") {
        let t = rest
            .strip_suffix(']')
            .ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        return Ok(Step::SiblingsT(t.to_string()));
    }

    if let Some(rest) = token.strip_prefix("children[") {
        let t = rest
            .strip_suffix(']')
            .ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        return Ok(Step::ChildrenT(t.to_string()));
    }

    if let Some(rest) = token.strip_prefix("up[") {
        let t = rest
            .strip_suffix(']')
            .ok_or_else(|| ParseError::UnknownStep(token.to_string()))?;
        return Ok(Step::UpT(t.to_string()));
    }

    Err(ParseError::UnknownStep(token.to_string()))
}
```

- [ ] **Step 4: Update `src/traversal/mod.rs`**

```rust
pub mod executor;
pub mod pipeline;
```

- [ ] **Step 5: Run tests — expect all pass**

```bash
cargo test traversal::pipeline
```

Expected: 13 tests PASS, 0 failed.

- [ ] **Step 6: Verify compile**

```bash
cargo check
```

Expected: errors about `parser` module not found (will be fixed in Task 3). If only those errors appear, the parser itself is correct.

- [ ] **Step 7: Commit**

```bash
git add src/traversal/pipeline.rs src/traversal/mod.rs
git commit -m "feat: add ORGTRVLANG v2 pipeline parser with chaining support"
```

---

## Task 2: Executor Rewrite

**Files:**
- Replace: `src/traversal/executor.rs`

- [ ] **Step 1: Write the new `src/traversal/executor.rs`**

Replace the entire file with:

```rust
use std::collections::HashSet;

use sqlx::PgPool;
use uuid::Uuid;

use crate::{error::AppError, models::Orgu};
use super::pipeline::{Pipeline, Step};

const COLS: &str = "orgu_id, orgtnt_id, orgt_id, parent_orgu_id, \
                    path::text AS path, orgu_t, name, metadata, \
                    is_active, created_at, updated_at";

pub async fn execute(
    pool:    &PgPool,
    anchor:  Uuid,
    orgt_id: Uuid,
    pipeline: &Pipeline,
) -> Result<Vec<Orgu>, AppError> {
    let mut current_ids: Vec<Uuid> = vec![anchor];

    if pipeline.steps.is_empty() {
        return fetch_by_ids(pool, &current_ids, orgt_id).await;
    }

    let mut last_result: Vec<Orgu> = vec![];
    for step in &pipeline.steps {
        last_result = execute_step(pool, &current_ids, orgt_id, step).await?;
        current_ids = dedup_ids(&last_result);
        if current_ids.is_empty() {
            return Ok(vec![]);
        }
    }

    Ok(last_result)
}

fn dedup_ids(rows: &[Orgu]) -> Vec<Uuid> {
    let mut seen = HashSet::new();
    rows.iter()
        .filter(|r| seen.insert(r.orgu_id))
        .map(|r| r.orgu_id)
        .collect()
}

async fn fetch_by_ids(
    pool:    &PgPool,
    ids:     &[Uuid],
    orgt_id: Uuid,
) -> Result<Vec<Orgu>, AppError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "SELECT {COLS} FROM orgu \
         WHERE orgu_id = ANY($1::uuid[]) AND orgt_id = $2 AND is_active = true"
    ))
    .bind(ids)
    .bind(orgt_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::Database)
}

async fn execute_step(
    pool:    &PgPool,
    ids:     &[Uuid],
    orgt_id: Uuid,
    step:    &Step,
) -> Result<Vec<Orgu>, AppError> {
    match step {
        Step::Children => {
            sqlx::query_as::<_, Orgu>(&format!(
                "SELECT {COLS} FROM orgu \
                 WHERE parent_orgu_id = ANY($1::uuid[]) \
                   AND orgt_id = $2 AND is_active = true"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }

        Step::ChildrenT(t) => {
            sqlx::query_as::<_, Orgu>(&format!(
                "SELECT {COLS} FROM orgu \
                 WHERE parent_orgu_id = ANY($1::uuid[]) \
                   AND orgu_t = $3 AND orgt_id = $2 AND is_active = true"
            ))
            .bind(ids)
            .bind(orgt_id)
            .bind(t.as_str())
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }

        Step::Parent => {
            sqlx::query_as::<_, Orgu>(&format!(
                "SELECT DISTINCT {COLS} FROM orgu \
                 WHERE orgu_id IN ( \
                     SELECT parent_orgu_id FROM orgu \
                     WHERE orgu_id = ANY($1::uuid[]) AND parent_orgu_id IS NOT NULL \
                 ) AND orgt_id = $2 AND is_active = true"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }

        Step::Siblings => {
            sqlx::query_as::<_, Orgu>(&format!(
                "SELECT DISTINCT {COLS} FROM orgu \
                 WHERE parent_orgu_id IN ( \
                     SELECT parent_orgu_id FROM orgu \
                     WHERE orgu_id = ANY($1::uuid[]) AND parent_orgu_id IS NOT NULL \
                 ) \
                 AND orgu_id != ALL($1::uuid[]) \
                 AND orgt_id = $2 AND is_active = true"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }

        Step::SiblingsT(t) => {
            sqlx::query_as::<_, Orgu>(&format!(
                "SELECT DISTINCT {COLS} FROM orgu \
                 WHERE parent_orgu_id IN ( \
                     SELECT parent_orgu_id FROM orgu \
                     WHERE orgu_id = ANY($1::uuid[]) AND parent_orgu_id IS NOT NULL \
                 ) \
                 AND orgu_id != ALL($1::uuid[]) \
                 AND orgu_t = $3 AND orgt_id = $2 AND is_active = true"
            ))
            .bind(ids)
            .bind(orgt_id)
            .bind(t.as_str())
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }

        Step::UpT(t) => {
            // For each anchor, find the nearest ancestor of type T.
            // DISTINCT ON (anchor.orgu_id) + ORDER BY nlevel DESC picks the deepest ancestor.
            // Outer SELECT DISTINCT removes duplicates when multiple anchors share an ancestor.
            sqlx::query_as::<_, Orgu>(
                "SELECT DISTINCT anc.orgu_id, anc.orgtnt_id, anc.orgt_id, anc.parent_orgu_id, \
                        anc.path::text AS path, anc.orgu_t, anc.name, anc.metadata, \
                        anc.is_active, anc.created_at, anc.updated_at \
                 FROM ( \
                     SELECT DISTINCT ON (anchor.orgu_id) anc.* \
                     FROM (SELECT orgu_id, path FROM orgu WHERE orgu_id = ANY($1::uuid[])) AS anchor \
                     JOIN orgu anc \
                       ON anchor.path <@ anc.path \
                      AND anc.orgu_t = $3 \
                      AND anc.orgt_id = $2 \
                      AND anc.is_active = true \
                      AND anc.path <> anchor.path \
                     ORDER BY anchor.orgu_id, nlevel(anc.path) DESC \
                 ) anc"
            )
            .bind(ids)
            .bind(orgt_id)
            .bind(t.as_str())
            .fetch_all(pool)
            .await
            .map_err(AppError::Database)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traversal::pipeline::parse;

    const CANKAYA_ID:    &str = "00000000-0000-0000-0001-000000000004"; // 1.2.3.4 sube
    const ANKARA_IL_ID:  &str = "00000000-0000-0000-0001-000000000003"; // 1.2.3   il
    const IC_ANADOLU_ID: &str = "00000000-0000-0000-0001-000000000002"; // 1.2     bolge
    const ORGT_ID:       &str = "00000000-0000-0000-0000-000000000010";

    async fn make_pool() -> PgPool {
        dotenvy::dotenv().ok();
        let url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set for integration tests");
        PgPool::connect(&url).await.expect("DB connection failed")
    }

    fn uid(s: &str) -> Uuid { Uuid::parse_str(s).unwrap() }

    fn names(rows: &[Orgu]) -> Vec<&str> {
        let mut v: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        v.sort_unstable();
        v
    }

    async fn run(pool: &PgPool, anchor: &str, expr: &str) -> Vec<Orgu> {
        let pipeline = parse(expr).unwrap();
        execute(pool, uid(anchor), uid(ORGT_ID), &pipeline).await.unwrap()
    }

    #[tokio::test]
    async fn test_self() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self").await;
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "Çankaya Şubesi");
    }

    #[tokio::test]
    async fn test_parent() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.parent").await;
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "Ankara İl Md.");
    }

    #[tokio::test]
    async fn test_siblings() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.siblings").await;
        assert_eq!(names(&res), vec!["Keçiören Şubesi", "Kredi Değerlendirme Ankara"]);
    }

    #[tokio::test]
    async fn test_siblings_t() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.siblings[sube]").await;
        assert_eq!(names(&res), vec!["Keçiören Şubesi"]);
    }

    #[tokio::test]
    async fn test_children() {
        let pool = make_pool().await;
        let res = run(&pool, ANKARA_IL_ID, "self.children").await;
        assert_eq!(res.len(), 3);
        assert!(names(&res).contains(&"Çankaya Şubesi"));
        assert!(names(&res).contains(&"Keçiören Şubesi"));
        assert!(names(&res).contains(&"Kredi Değerlendirme Ankara"));
    }

    #[tokio::test]
    async fn test_children_t() {
        let pool = make_pool().await;
        let res = run(&pool, ANKARA_IL_ID, "self.children[sube]").await;
        assert_eq!(res.len(), 2);
        assert!(names(&res).contains(&"Çankaya Şubesi"));
        assert!(names(&res).contains(&"Keçiören Şubesi"));
    }

    #[tokio::test]
    async fn test_up_t() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.up[bolge]").await;
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].name, "İç Anadolu Bölge Md.");
    }

    #[tokio::test]
    async fn test_up_t_children() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.up[bolge].children").await;
        assert_eq!(res.len(), 2);
        assert!(names(&res).contains(&"Ankara İl Md."));
        assert!(names(&res).contains(&"Eskişehir İl Md."));
    }

    #[tokio::test]
    async fn test_up_t_children_t() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.up[bolge].children[il]").await;
        assert_eq!(res.len(), 2);
        assert!(names(&res).contains(&"Ankara İl Md."));
        assert!(names(&res).contains(&"Eskişehir İl Md."));
    }

    #[tokio::test]
    async fn test_three_step_chain() {
        let pool = make_pool().await;
        let res = run(&pool, CANKAYA_ID, "self.up[bolge].children[il].children[sube]").await;
        assert_eq!(res.len(), 4);
        assert!(names(&res).contains(&"Çankaya Şubesi"));
        assert!(names(&res).contains(&"Keçiören Şubesi"));
        assert!(names(&res).contains(&"Odunpazarı Şubesi"));
        assert!(names(&res).contains(&"Tepebaşı Şubesi"));
    }

    #[tokio::test]
    async fn test_siblings_then_children() {
        // anchor = Ankara İl (1.2.3)
        // siblings = Eskişehir İl (1.2.7)
        // siblings.children[sube] = Odunpazarı (1.2.7.8), Tepebaşı (1.2.7.9)
        let pool = make_pool().await;
        let res = run(&pool, ANKARA_IL_ID, "self.siblings.children[sube]").await;
        assert_eq!(res.len(), 2);
        assert!(names(&res).contains(&"Odunpazarı Şubesi"));
        assert!(names(&res).contains(&"Tepebaşı Şubesi"));
    }

    #[tokio::test]
    async fn test_empty_result_propagation() {
        // Root has no parent → empty result
        let pool = make_pool().await;
        let root_id = "00000000-0000-0000-0001-000000000001";
        let res = run(&pool, root_id, "self.parent").await;
        assert_eq!(res.len(), 0);
    }
}
```

- [ ] **Step 2: Verify compile (expect errors about `parser` — OK for now)**

```bash
cargo check 2>&1 | grep -v "error\[E0432\]" | head -20
```

The executor itself should have no new errors beyond the `parser` module missing. Confirm `executor.rs` compiles cleanly by checking the error list is only about `parser`.

- [ ] **Step 3: Run integration tests**

```bash
cargo test traversal::executor::tests
```

Expected: 12 tests PASS. If any fail, read the assertion error — the expected values come directly from the `init.sql` comments.

- [ ] **Step 4: Commit**

```bash
git add src/traversal/executor.rs
git commit -m "feat: rewrite executor with batch SQL pipeline execution"
```

---

## Task 3: Wire Handler + Cleanup

**Files:**
- Delete: `src/traversal/parser.rs`
- Modify: `src/handlers/traverse.rs`

- [ ] **Step 1: Delete the old parser**

```bash
rm /home/alphan/Desktop/workflow-engine/org-api/src/traversal/parser.rs
```

- [ ] **Step 2: Replace `src/handlers/traverse.rs`**

```rust
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    error::AppError,
    models::Orgu,
    traversal::{executor, pipeline},
};

#[derive(Deserialize)]
pub struct TraverseQuery {
    pub expr: String,
}

pub async fn traverse_orgu(
    State(pool): State<PgPool>,
    Path(orgu_id): Path<Uuid>,
    Query(params): Query<TraverseQuery>,
) -> Result<Json<Vec<Orgu>>, AppError> {
    let pipeline = pipeline::parse(&params.expr)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let orgt_id: Uuid =
        sqlx::query_scalar::<_, Uuid>("SELECT orgt_id FROM orgu WHERE orgu_id = $1")
            .bind(orgu_id)
            .fetch_optional(&pool)
            .await?
            .ok_or_else(|| AppError::NotFound(format!("orgu not found: {orgu_id}")))?;

    let result = executor::execute(&pool, orgu_id, orgt_id, &pipeline).await?;
    Ok(Json(result))
}
```

- [ ] **Step 3: Build**

```bash
cargo build
```

Expected: **zero errors**, only the usual dead-code warnings. If there are errors, they will be about the `parser` module — verify `src/traversal/mod.rs` no longer references `parser`.

- [ ] **Step 4: Run all tests**

```bash
cargo test
```

Expected: all 25 tests pass (13 pipeline unit tests + 12 executor integration tests).

- [ ] **Step 5: Commit**

```bash
git add src/handlers/traverse.rs src/traversal/mod.rs
git rm src/traversal/parser.rs
git commit -m "feat: wire v2 pipeline parser into traverse handler, remove v1 parser"
```

---

## Task 4: Smoke Test

**Files:** None (curl only)

Start the server:

```bash
cd /home/alphan/Desktop/workflow-engine/org-api
cargo run &
sleep 2
```

- [ ] **Step 1: Legacy-equivalent expressions (must still work)**

```bash
BASE="http://localhost:3000"
CANKAYA="00000000-0000-0000-0001-000000000004"
ANKARA="00000000-0000-0000-0001-000000000003"
IC_ANADOLU="00000000-0000-0000-0001-000000000002"

# self → Çankaya
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self" | jq '[.[] | .name]'
# Expected: ["Çankaya Şubesi"]

# self.parent → Ankara İl Md.
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self.parent" | jq '[.[] | .name]'
# Expected: ["Ankara İl Md."]

# self.siblings[sube] → Keçiören only
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self.siblings%5Bsube%5D" | jq '[.[] | .name]'
# Expected: ["Keçiören Şubesi"]

# self.up[bolge].children[il] → 2 il nodes
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self.up%5Bbolge%5D.children%5Bil%5D" | jq '[.[] | .name]'
# Expected: ["Ankara İl Md.", "Eskişehir İl Md."]

# self.children[il].children[sube] → 4 sube (anchor = İç Anadolu Bölge, has il children)
curl -s "$BASE/orgu/$IC_ANADOLU/traverse?expr=self.children%5Bil%5D.children%5Bsube%5D" | jq '[.[] | .name]'
# Expected: ["Çankaya Şubesi","Keçiören Şubesi","Odunpazarı Şubesi","Tepebaşı Şubesi"]
```

- [ ] **Step 2: NEW — three-step chain (impossible in v1)**

```bash
# self.up[bolge].children[il].children[sube] — 3 steps from Çankaya
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self.up%5Bbolge%5D.children%5Bil%5D.children%5Bsube%5D" | jq '[.[] | .name]'
# Expected: ["Çankaya Şubesi","Keçiören Şubesi","Odunpazarı Şubesi","Tepebaşı Şubesi"]
```

- [ ] **Step 3: NEW — siblings then children (impossible in v1)**

```bash
# self.siblings.children[sube] from Ankara İl → Eskişehir'in sube çocukları
curl -s "$BASE/orgu/$ANKARA/traverse?expr=self.siblings.children%5Bsube%5D" | jq '[.[] | .name]'
# Expected: ["Odunpazarı Şubesi","Tepebaşı Şubesi"]
```

- [ ] **Step 4: Error cases**

```bash
# Old v1 syntax without self → 400
curl -s -o /dev/null -w "%{http_code}" "$BASE/orgu/$CANKAYA/traverse?expr=up%5Bbolge%5D"
# Expected: 400

# Unknown step → 400
curl -s "$BASE/orgu/$CANKAYA/traverse?expr=self.garbage" | jq .
# Expected: {"error": "unknown step: \"garbage\""}
```

- [ ] **Step 5: Kill server and final commit**

```bash
kill %1

# Only if there are unstaged changes:
git add -A
git commit -m "chore: ORGTRVLANG v2 smoke tests passing — pipeline language complete"
```

---

## Summary

| Task | Deliverable |
|------|------------|
| 1 | `pipeline.rs` — composable AST parser, 13 unit tests |
| 2 | `executor.rs` — set-based batch SQL execution, 12 integration tests |
| 3 | Handler wired to v2, `parser.rs` deleted, full build passes |
| 4 | Smoke tests confirming new chained expressions work |
