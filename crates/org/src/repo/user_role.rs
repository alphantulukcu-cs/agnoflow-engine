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
    type_expr: &str,
    orgtnt_id: Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
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
