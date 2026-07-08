use sqlx::PgPool;
use uuid::Uuid;
use crate::{
    error::OrgError,
    models::{OrgUnit, Role, User, UserOrgu, UserRole},
    traversal::{executor, parser},
};

pub async fn list_users(pool: &PgPool, orgtnt_id: Uuid, limit: i64, offset: i64) -> Result<Vec<User>, OrgError> {
    sqlx::query_as::<_, User>(
        "SELECT u_id, orgtnt_id, username, full_name, email, is_active, created_at
         FROM org.u
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY full_name, username LIMIT $2 OFFSET $3"
    )
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_roles(pool: &PgPool, orgtnt_id: Uuid, limit: i64, offset: i64) -> Result<Vec<Role>, OrgError> {
    sqlx::query_as::<_, Role>(
        "SELECT r_id, orgtnt_id, name, display_name, is_active, created_at
         FROM org.r
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY display_name, name LIMIT $2 OFFSET $3"
    )
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_user_orgus(pool: &PgPool, user_id: Uuid, limit: i64, offset: i64) -> Result<Vec<UserOrgu>, OrgError> {
    sqlx::query_as::<_, UserOrgu>(
        "SELECT u_orgu_id, orgtnt_id, u_id, orgu_id, is_primary, created_at
         FROM org.u_orgu
         WHERE u_id = $1
         ORDER BY is_primary DESC, created_at LIMIT $2 OFFSET $3"
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_user_roles(pool: &PgPool, user_id: Uuid, limit: i64, offset: i64) -> Result<Vec<UserRole>, OrgError> {
    sqlx::query_as::<_, UserRole>(
        "SELECT ur.ur_id, ur.orgtnt_id, ur.u_id, ur.r_id, r.name AS role_name,
                ur.orgu_id, ur.orgu_scope, ur.ur_type, ur.valid_from, ur.valid_until, ur.created_at
         FROM org.ur ur
         JOIN org.r r ON ur.r_id = r.r_id
         WHERE ur.u_id = $1
           AND ur.ur_type != 'excluded'
           AND (ur.valid_from IS NULL OR ur.valid_from <= now())
           AND (ur.valid_until IS NULL OR ur.valid_until > now())
         ORDER BY r.name, ur.created_at LIMIT $2 OFFSET $3"
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

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
               AND r.is_active = true
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
