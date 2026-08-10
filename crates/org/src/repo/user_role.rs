use crate::{
    error::OrgError,
    models::{OrgUnit, Role, User, UserOrgu, UserRole},
    traversal::{executor, parser},
};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn list_users(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<User>, OrgError> {
    sqlx::query_as::<_, User>(
        "SELECT u_id, orgtnt_id, username, full_name, email, is_active, created_at
         FROM org.u
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY full_name, username LIMIT $2 OFFSET $3",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// `LIKE` joker karakterlerini kaçırır — kullanıcının yazdığı `%` ve `_` ARAMA METNİDİR,
/// desen değil. Kaçırılmazsa tek bir `%` tüm tenant'ı döndürür (ve indeksi işe yaramaz
/// kılar), `_` de her harfe uyar.
fn escape_like(input: &str) -> String {
    input
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Kullanıcı arama — editörün `c_u` tamamlaması buradan beslenir.
///
/// İstemci tarafı süzme YETMEZ: on binlerce kullanıcılı bir tenant'ta tüm listeyi
/// indirmek gerekirdi. Arama kullanıcı adı / tam ad / e-posta üzerinde `ILIKE '%q%'`
/// (trigram GIN indeksi, `migrations/org/20260807000001_user_search.sql`); `q` bir UUID
/// ise `u_id` eşitliği de denenir — `c_u` alanı kullanıcı adını da UUID'yi de kabul
/// ediyor (`wfe-core/src/v22/matcher.rs`), arama ikisini de bulabilmeli.
///
/// Sıra ÖNEK eşleşmesini öne alır: "ah" yazan kullanıcı `ahmet`i `mahmut`tan önce görür.
pub async fn search_users(
    pool: &PgPool,
    orgtnt_id: Uuid,
    query: &str,
    limit: i64,
) -> Result<Vec<User>, OrgError> {
    let escaped = escape_like(query.trim());
    let contains = format!("%{escaped}%");
    let prefix = format!("{escaped}%");
    // Geçersiz UUID metni HATA DEĞİLDİR: sıradan bir arama terimidir, `u_id` kanalı kapanır.
    let as_uuid = Uuid::parse_str(query.trim()).ok();

    sqlx::query_as::<_, User>(
        r#"SELECT u_id, orgtnt_id, username, full_name, email, is_active, created_at
           FROM org.u
           WHERE orgtnt_id = $1 AND is_active = true
             AND (username ILIKE $2 ESCAPE '\'
               OR full_name ILIKE $2 ESCAPE '\'
               OR coalesce(email, '') ILIKE $2 ESCAPE '\'
               OR u_id = $4)
           ORDER BY (CASE WHEN username ILIKE $3 ESCAPE '\'
                            OR full_name ILIKE $3 ESCAPE '\' THEN 0 ELSE 1 END),
                    full_name, username
           LIMIT $5"#,
    )
    .bind(orgtnt_id)
    .bind(&contains)
    .bind(&prefix)
    .bind(as_uuid)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_roles(
    pool: &PgPool,
    orgtnt_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Role>, OrgError> {
    sqlx::query_as::<_, Role>(
        "SELECT r_id, orgtnt_id, name, display_name, is_active, created_at
         FROM org.r
         WHERE orgtnt_id = $1 AND is_active = true
         ORDER BY display_name, name LIMIT $2 OFFSET $3",
    )
    .bind(orgtnt_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

#[cfg(test)]
mod tests {
    use super::escape_like;

    #[test]
    fn like_wildcards_are_literal_search_text() {
        // Kaçırılmasaydı bu terim TÜM tenant'ı döndürürdü.
        assert_eq!(escape_like("%"), "\\%");
        assert_eq!(escape_like("a_b"), "a\\_b");
        // Ters bölü ÖNCE kaçırılmalı, yoksa kendi eklediğimiz kaçışları bozar.
        assert_eq!(escape_like("a\\%"), "a\\\\\\%");
        assert_eq!(escape_like("ahmet.yilmaz"), "ahmet.yilmaz");
    }
}

pub async fn list_user_orgus(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<UserOrgu>, OrgError> {
    sqlx::query_as::<_, UserOrgu>(
        "SELECT u_orgu_id, orgtnt_id, u_id, orgu_id, is_primary, created_at
         FROM org.u_orgu
         WHERE u_id = $1
         ORDER BY is_primary DESC, created_at LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn list_user_roles(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<UserRole>, OrgError> {
    sqlx::query_as::<_, UserRole>(
        "SELECT ur.ur_id, ur.orgtnt_id, ur.u_id, ur.r_id, r.name AS role_name,
                ur.orgu_id, ur.orgu_scope, ur.ur_type, ur.valid_from, ur.valid_until, ur.created_at
         FROM org.ur ur
         JOIN org.r r ON ur.r_id = r.r_id
         WHERE ur.u_id = $1
           AND ur.ur_type != 'excluded'
           AND (ur.valid_from IS NULL OR ur.valid_from <= now())
           AND (ur.valid_until IS NULL OR ur.valid_until > now())
         ORDER BY r.name, ur.created_at LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Kullanıcı verilen orgu'da verilen role sahip mi?
/// Kaynak: org.ur doğrudan grant VEYA org.orgu_r birim grant'ı (devralma);
/// org.ur 'excluded' satırı her ikisini de ezer. Timeslice geçerliliği uygulanır.
pub async fn check_user_role(
    pool: &PgPool,
    user_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<bool, OrgError> {
    let exists = sqlx::query_scalar::<_, bool>(
        "SELECT
            ( EXISTS (
                SELECT 1 FROM org.ur u JOIN org.r r ON u.r_id = r.r_id
                WHERE u.u_id = $1 AND u.orgu_id = $2 AND r.name = $3
                  AND r.is_active = true AND u.ur_type != 'excluded'
                  AND (u.valid_from  IS NULL OR u.valid_from  <= now())
                  AND (u.valid_until IS NULL OR u.valid_until >  now()) )
              OR EXISTS (
                SELECT 1 FROM org.orgu_r orr JOIN org.r r ON orr.r_id = r.r_id
                WHERE orr.orgu_id = $2 AND r.name = $3
                  AND r.is_active = true
                  AND (orr.valid_from  IS NULL OR orr.valid_from  <= now())
                  AND (orr.valid_until IS NULL OR orr.valid_until >  now()) ) )
          AND NOT EXISTS (
                SELECT 1 FROM org.ur u JOIN org.r r ON u.r_id = r.r_id
                WHERE u.u_id = $1 AND u.orgu_id = $2 AND r.name = $3
                  AND u.ur_type = 'excluded' )",
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
    pool: &PgPool,
    anchor_orgu_id: Uuid,
    expr: &str,
    orgtnt_id: Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
    if let Some(type_expr) = expr.strip_prefix("*:") {
        return resolve_global_type(pool, type_expr, orgtnt_id).await;
    }

    let orgt_id = super::orgu::get_orgt_id(pool, anchor_orgu_id).await?;
    let pipeline = parser::parse(expr).map_err(|e| OrgError::BadRequest(e.to_string()))?;
    let orgus = executor::execute(pool, anchor_orgu_id, orgt_id, orgtnt_id, &pipeline).await?;
    Ok(orgus.into_iter().map(OrgUnit::from).collect())
}

/// `*:[filter]` — tenant genelinde filtreye uyan orgu üyelikleri.
/// Executor'ın filtre motoruna delege eder (type/role, &&/||, virgül-OR hepsi desteklenir).
async fn resolve_global_type(
    pool: &PgPool,
    type_expr: &str,
    orgtnt_id: Uuid,
) -> Result<Vec<OrgUnit>, OrgError> {
    let inner = type_expr
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    let filter = parser::parse_filter(inner).map_err(|e| OrgError::BadRequest(e.to_string()))?;
    let orgus = executor::fetch_global_type(pool, orgtnt_id, &filter).await?;
    Ok(orgus.into_iter().map(OrgUnit::from).collect())
}

// ── Simülasyon playground: aktör oluşturma (WOR sim) ────────────────────────
// Bu fonksiyonlar yalnızca sim/dev senaryosu içindir: kritere uygun hazır aktör
// yoksa UI'dan (orgu_id + rol) eşleşen bir aktör üretmeye izin verir.

/// Yeni kullanıcı ekler. (orgtnt_id, username) benzersizdir → varsa hata döner.
pub async fn create_user(
    pool: &PgPool,
    orgtnt_id: Uuid,
    username: &str,
    full_name: &str,
    email: Option<&str>,
) -> Result<User, OrgError> {
    sqlx::query_as::<_, User>(
        "INSERT INTO org.u (orgtnt_id, username, full_name, email)
         VALUES ($1, $2, $3, $4)
         RETURNING u_id, orgtnt_id, username, full_name, email, is_active, created_at",
    )
    .bind(orgtnt_id)
    .bind(username)
    .bind(full_name)
    .bind(email)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn update_user(pool: &PgPool, user_id: Uuid, full_name: &str) -> Result<User, OrgError> {
    sqlx::query_as::<_, User>(
        "UPDATE org.u
         SET full_name = $2
         WHERE u_id = $1 AND is_active = true
         RETURNING u_id, orgtnt_id, username, full_name, email, is_active, created_at",
    )
    .bind(user_id)
    .bind(full_name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Rolü ada göre bulur; yoksa oluşturur (display_name = name).
pub async fn ensure_role(pool: &PgPool, orgtnt_id: Uuid, name: &str) -> Result<Role, OrgError> {
    if let Some(r) = sqlx::query_as::<_, Role>(
        "SELECT r_id, orgtnt_id, name, display_name, is_active, created_at
         FROM org.r WHERE orgtnt_id = $1 AND name = $2",
    )
    .bind(orgtnt_id)
    .bind(name)
    .fetch_optional(pool)
    .await
    .map_err(OrgError::Database)?
    {
        return Ok(r);
    }
    sqlx::query_as::<_, Role>(
        "INSERT INTO org.r (orgtnt_id, name, display_name)
         VALUES ($1, $2, $2)
         RETURNING r_id, orgtnt_id, name, display_name, is_active, created_at",
    )
    .bind(orgtnt_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Yeni rol ekler. Aynı `name` varsa mevcut aktif rol döner; display name boşsa name kullanılır.
pub async fn create_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    name: &str,
    display_name: Option<&str>,
) -> Result<Role, OrgError> {
    let display_name = display_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(name);

    sqlx::query_as::<_, Role>(
        "INSERT INTO org.r (orgtnt_id, name, display_name)
         VALUES ($1, $2, $3)
         ON CONFLICT (orgtnt_id, name) DO UPDATE
         SET display_name = EXCLUDED.display_name,
             is_active = true
         RETURNING r_id, orgtnt_id, name, display_name, is_active, created_at",
    )
    .bind(orgtnt_id)
    .bind(name)
    .bind(display_name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Rolün teknik adını ve görünen adını günceller. Atamalar r_id ile bağlı olduğu
/// için bu değişiklik rolün kullanıldığı tüm kullanıcı/birim kayıtlarına yansır.
pub async fn update_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    r_id: Uuid,
    name: &str,
    display_name: &str,
) -> Result<Role, OrgError> {
    sqlx::query_as::<_, Role>(
        "UPDATE org.r
         SET name = $4,
             display_name = $5,
             is_active = true
         WHERE orgtnt_id = $1 AND r_id = $2 AND is_active = $3
         RETURNING r_id, orgtnt_id, name, display_name, is_active, created_at",
    )
    .bind(orgtnt_id)
    .bind(r_id)
    .bind(true)
    .bind(name)
    .bind(display_name)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// (kullanıcı, birim, rol) atamasını garantiler: u_orgu üyeliği + granted ur satırı.
/// İdempotent — aynı atama tekrar çağrılırsa yeni satır eklenmez.
pub async fn grant_assignment(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<(), OrgError> {
    let role = ensure_role(pool, orgtnt_id, role_name).await?;

    sqlx::query(
        "INSERT INTO org.u_orgu (orgtnt_id, u_id, orgu_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (u_id, orgu_id) DO NOTHING",
    )
    .bind(orgtnt_id)
    .bind(u_id)
    .bind(orgu_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;

    sqlx::query(
        "INSERT INTO org.ur (orgtnt_id, u_id, r_id, orgu_id, ur_type)
         SELECT $1, $2, $3, $4, 'granted'
         WHERE NOT EXISTS (
             SELECT 1 FROM org.ur
             WHERE orgtnt_id = $1 AND u_id = $2 AND r_id = $3
               AND orgu_id = $4 AND ur_type = 'granted'
         )",
    )
    .bind(orgtnt_id)
    .bind(u_id)
    .bind(role.r_id)
    .bind(orgu_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;

    Ok(())
}

/// `grant_assignment`'ın tersi: bir kullanıcının bir orgu'daki 'granted' rol
/// atamasını (org.ur) kaldırır. Org üyeliği (org.u_orgu) KORUNUR — kullanıcı
/// başka rollerle o birimde kalabilir. Etkilenen satır varsa true.
pub async fn revoke_assignment(
    pool: &PgPool,
    orgtnt_id: Uuid,
    u_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<bool, OrgError> {
    let r = sqlx::query(
        "DELETE FROM org.ur
         WHERE orgtnt_id = $1 AND u_id = $2 AND orgu_id = $3 AND ur_type = 'granted'
           AND r_id = (SELECT r_id FROM org.r WHERE orgtnt_id = $1 AND name = $4)",
    )
    .bind(orgtnt_id)
    .bind(u_id)
    .bind(orgu_id)
    .bind(role_name)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(r.rows_affected() > 0)
}

/// Bir orgu'ya rol grant'ı: org.orgu_r satırı. Rol yoksa oluşturulur (ensure_role).
/// İdempotent — aynı (orgu, rol) tekrar çağrılırsa yeni satır eklenmez.
pub async fn grant_orgu_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<(), OrgError> {
    let role = ensure_role(pool, orgtnt_id, role_name).await?;
    sqlx::query(
        "INSERT INTO org.orgu_r (orgtnt_id, orgu_id, r_id, ur_type)
         VALUES ($1, $2, $3, 'granted')
         ON CONFLICT (orgu_id, r_id) DO NOTHING",
    )
    .bind(orgtnt_id)
    .bind(orgu_id)
    .bind(role.r_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(())
}

/// `grant_orgu_role`'ün tersi: bir orgu'nun rol grant'ını (org.orgu_r) kaldırır.
/// Etkilenen satır varsa true.
pub async fn revoke_orgu_role(
    pool: &PgPool,
    orgtnt_id: Uuid,
    orgu_id: Uuid,
    role_name: &str,
) -> Result<bool, OrgError> {
    let r = sqlx::query(
        "DELETE FROM org.orgu_r
         WHERE orgtnt_id = $1 AND orgu_id = $2
           AND r_id = (SELECT r_id FROM org.r WHERE orgtnt_id = $1 AND name = $3)",
    )
    .bind(orgtnt_id)
    .bind(orgu_id)
    .bind(role_name)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(r.rows_affected() > 0)
}
