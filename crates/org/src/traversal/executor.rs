use std::collections::HashSet;

use sqlx::postgres::PgArguments;
use sqlx::{Arguments, PgPool};
use uuid::Uuid;

use crate::{error::OrgError, models::Orgu};
use super::parser::{FilterExpr, Pipeline, Step};

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

const SEL: &str =
    "m.orgu_id, m.orgtnt_id, m.orgt_id, m.parent_orgu_id, \
     m.path::text AS path, m.orgu_type, m.name, m.metadata, \
     m.is_active, m.created_at, m.updated_at";

pub async fn execute(
    pool:     &PgPool,
    anchor:   Uuid,
    orgt_id:  Uuid,
    pipeline: &Pipeline,
) -> Result<Vec<Orgu>, OrgError> {
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
) -> Result<Vec<Orgu>, OrgError> {
    sqlx::query_as::<_, Orgu>(&format!(
        "{MEMBERS} \
         SELECT DISTINCT {SEL} FROM members m \
         WHERE m.orgu_id = ANY($1::uuid[])"
    ))
    .bind(ids)
    .bind(orgt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

fn filter_sql(expr: &FilterExpr, idx: &mut usize) -> (String, Vec<(String, String)>) {
    match expr {
        FilterExpr::Leaf(tf) => {
            let k = *idx; *idx += 1;
            let v = *idx; *idx += 1;
            let sql = format!(
                "(m.orgu_type->>${} = ${} OR m.orgu_type->${} @> to_jsonb(${}::text))",
                k, v, k, v
            );
            (sql, vec![(tf.key.clone(), tf.val.clone())])
        }
        FilterExpr::Not(inner) => {
            let (s, b) = filter_sql(inner, idx);
            (format!("NOT {s}"), b)
        }
        FilterExpr::And(exprs) => {
            let (parts, binds) = collect_filter_parts(exprs, idx);
            (format!("({})", parts.join(" AND ")), binds)
        }
        FilterExpr::Or(exprs) => {
            let (parts, binds) = collect_filter_parts(exprs, idx);
            (format!("({})", parts.join(" OR ")), binds)
        }
    }
}

fn collect_filter_parts(
    exprs: &[FilterExpr],
    idx:   &mut usize,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut parts = Vec::new();
    let mut binds = Vec::new();
    for e in exprs {
        let (s, b) = filter_sql(e, idx);
        parts.push(s);
        binds.extend(b);
    }
    (parts, binds)
}

async fn run_filtered(
    pool:     &PgPool,
    sql:      String,
    ids:      &[Uuid],
    orgt_id:  Uuid,
    bindings: Vec<(String, String)>,
) -> Result<Vec<Orgu>, OrgError> {
    let mut args = PgArguments::default();
    args.add(ids.to_vec());
    args.add(orgt_id);
    for (k, v) in bindings {
        args.add(k);
        args.add(v);
    }
    sqlx::query_as_with::<_, Orgu, _>(&sql, args)
        .fetch_all(pool)
        .await
        .map_err(OrgError::Database)
}

async fn execute_step(
    pool:    &PgPool,
    ids:     &[Uuid],
    orgt_id: Uuid,
    step:    &Step,
) -> Result<Vec<Orgu>, OrgError> {
    match step {
        Step::Children => {
            sqlx::query_as::<_, Orgu>(&format!(
                "{MEMBERS}, \
                 anchors AS (SELECT path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a ON m.path <@ a.path AND nlevel(m.path) = nlevel(a.path) + 1"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(OrgError::Database)
        }

        Step::ChildrenT(expr) => {
            let mut idx = 3usize;
            let (fsql, bindings) = filter_sql(expr, &mut idx);
            let sql = format!(
                "{MEMBERS}, \
                 anchors AS (SELECT path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a ON m.path <@ a.path AND nlevel(m.path) = nlevel(a.path) + 1 \
                 WHERE (m.orgu_type ? '*' OR {fsql})"
            );
            run_filtered(pool, sql, ids, orgt_id, bindings).await
        }

        Step::Parent => {
            sqlx::query_as::<_, Orgu>(&format!(
                "{MEMBERS}, \
                 anchors AS (SELECT path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a ON m.path = subpath(a.path, 0, nlevel(a.path) - 1) \
                 WHERE nlevel(a.path) > 0"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(OrgError::Database)
        }

        Step::Siblings => {
            sqlx::query_as::<_, Orgu>(&format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a \
                   ON subpath(m.path, 0, nlevel(m.path) - 1) = subpath(a.path, 0, nlevel(a.path) - 1) \
                  AND m.path <> a.path"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(OrgError::Database)
        }

        Step::SiblingsT(expr) => {
            let mut idx = 3usize;
            let (fsql, bindings) = filter_sql(expr, &mut idx);
            let sql = format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a \
                   ON subpath(m.path, 0, nlevel(m.path) - 1) = subpath(a.path, 0, nlevel(a.path) - 1) \
                  AND m.path <> a.path \
                 WHERE (m.orgu_type ? '*' OR {fsql})"
            );
            run_filtered(pool, sql, ids, orgt_id, bindings).await
        }

        Step::UpT(expr) => {
            let mut idx = 3usize;
            let (fsql, bindings) = filter_sql(expr, &mut idx);
            let sql = format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM ( \
                     SELECT DISTINCT ON (a.orgu_id) m.* \
                     FROM anchors a \
                     JOIN members m ON a.path <@ m.path \
                       AND (m.orgu_type ? '*' OR {fsql}) \
                       AND m.path <> a.path \
                     ORDER BY a.orgu_id, nlevel(m.path) DESC \
                 ) m"
            );
            run_filtered(pool, sql, ids, orgt_id, bindings).await
        }

        Step::Ancestors => {
            sqlx::query_as::<_, Orgu>(&format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a ON a.path <@ m.path AND m.path <> a.path"
            ))
            .bind(ids)
            .bind(orgt_id)
            .fetch_all(pool)
            .await
            .map_err(OrgError::Database)
        }

        Step::AncestorsT(expr) => {
            let mut idx = 3usize;
            let (fsql, bindings) = filter_sql(expr, &mut idx);
            let sql = format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN anchors a ON a.path <@ m.path AND m.path <> a.path \
                 WHERE (m.orgu_type ? '*' OR {fsql})"
            );
            run_filtered(pool, sql, ids, orgt_id, bindings).await
        }

        Step::DownT(expr) => {
            let mut idx = 3usize;
            let (fsql, bindings) = filter_sql(expr, &mut idx);
            let sql = format!(
                "{MEMBERS}, \
                 anchors AS (SELECT orgu_id, path FROM members WHERE orgu_id = ANY($1::uuid[])), \
                 candidates AS ( \
                     SELECT m.orgu_id, m.path AS cpath, a.orgu_id AS anch_id \
                     FROM anchors a \
                     JOIN members m ON m.path <@ a.path AND m.path <> a.path \
                     WHERE (m.orgu_type ? '*' OR {fsql}) \
                 ) \
                 SELECT DISTINCT {SEL} FROM members m \
                 JOIN candidates c ON m.orgu_id = c.orgu_id \
                 WHERE NOT EXISTS ( \
                     SELECT 1 FROM candidates c2 \
                     WHERE c2.anch_id = c.anch_id \
                       AND c.cpath <@ c2.cpath \
                       AND c2.cpath <> c.cpath \
                 )"
            );
            run_filtered(pool, sql, ids, orgt_id, bindings).await
        }
    }
}
