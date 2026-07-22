//! Madde 6: vekalet/delegasyon repo. Aday tarama (`active_candidates`) bir ÜST
//! küme döner — kesin alıcı (grantee) eşleşmesi wfe-core matcher'ında yapılır.

use crate::error::OrgError;
use crate::models::Delegation;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

const COLS: &str = "delegation_id, orgtnt_id, delegator_user_id, seat_orgu_id, seat_role, \
                    grantee, valid_from, valid_to, active, created_by, created_at";

/// Yeni vekalet oluşturur. Koltuk sahipliği (delegator gerçekten seat_role'ü
/// taşıyor mu) ÇAĞIRAN katmanda (server route) `check_user_role` ile doğrulanır.
#[allow(clippy::too_many_arguments)]
pub async fn create(
    pool: &PgPool,
    orgtnt_id: Uuid,
    delegator_user_id: Uuid,
    seat_orgu_id: Uuid,
    seat_role: &str,
    grantee: &serde_json::Value,
    valid_from: DateTime<Utc>,
    valid_to: DateTime<Utc>,
    created_by: Uuid,
) -> Result<Delegation, OrgError> {
    sqlx::query_as::<_, Delegation>(&format!(
        "INSERT INTO org.delegation
           (orgtnt_id, delegator_user_id, seat_orgu_id, seat_role, grantee,
            valid_from, valid_to, created_by)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         RETURNING {COLS}"
    ))
    .bind(orgtnt_id)
    .bind(delegator_user_id)
    .bind(seat_orgu_id)
    .bind(seat_role)
    .bind(grantee)
    .bind(valid_from)
    .bind(valid_to)
    .bind(created_by)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Bir kullanıcının VERDİĞİ vekaletler (aktif + geçmiş), en yeni önce.
pub async fn list_by_delegator(
    pool: &PgPool,
    delegator_user_id: Uuid,
) -> Result<Vec<Delegation>, OrgError> {
    sqlx::query_as::<_, Delegation>(&format!(
        "SELECT {COLS} FROM org.delegation
         WHERE delegator_user_id = $1
         ORDER BY created_at DESC"
    ))
    .bind(delegator_user_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Tenant'taki TÜM vekaletler (admin yönetim görünümü), en yeni önce.
pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<Delegation>, OrgError> {
    sqlx::query_as::<_, Delegation>(&format!(
        "SELECT {COLS} FROM org.delegation
         WHERE orgtnt_id = $1
         ORDER BY created_at DESC"
    ))
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Matcher için aday aktif vekaletler: tenant + zaman penceresi + `active`, ve
/// kaba alıcı filtresi — grantee ya bir HAVUZ olabilir (`c_r` taşır, rol eşleşmesi
/// matcher'da) ya da bu claimant'ı ADIYLA (username/uuid) c_u'da içerir. Kesin
/// eşleşme `authorize(grantee, claimant)` ile matcher'da yapılır.
pub async fn active_candidates(
    pool: &PgPool,
    claimant_user_id: Uuid,
    orgtnt_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Vec<Delegation>, OrgError> {
    sqlx::query_as::<_, Delegation>(&format!(
        "SELECT {COLS} FROM org.delegation
         WHERE orgtnt_id = $1
           AND active
           AND valid_from <= $2
           AND valid_to   >  $2
           AND ( grantee ? 'c_r'
                 OR grantee->'c_u' ? (SELECT username FROM org.u WHERE u_id = $3)
                 OR grantee->'c_u' ? $3::text )"
    ))
    .bind(orgtnt_id)
    .bind(now)
    .bind(claimant_user_id)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

/// Tek vekaleti getirir (sahiplik kontrolü için).
pub async fn get_by_id(pool: &PgPool, delegation_id: Uuid) -> Result<Option<Delegation>, OrgError> {
    sqlx::query_as::<_, Delegation>(&format!(
        "SELECT {COLS} FROM org.delegation WHERE delegation_id = $1"
    ))
    .bind(delegation_id)
    .fetch_optional(pool)
    .await
    .map_err(OrgError::Database)
}

/// Vekaleti iptal eder (active=false). Tenant sınırı içinde; etkilenen satır varsa true.
pub async fn revoke(pool: &PgPool, delegation_id: Uuid, orgtnt_id: Uuid) -> Result<bool, OrgError> {
    let r = sqlx::query(
        "UPDATE org.delegation SET active = false, updated_at = now()
         WHERE delegation_id = $1 AND orgtnt_id = $2",
    )
    .bind(delegation_id)
    .bind(orgtnt_id)
    .execute(pool)
    .await
    .map_err(OrgError::Database)?;
    Ok(r.rows_affected() == 1)
}

/// Koltuk sahipliği doğrulaması için: delegator, seat_orgu'da seat_role'ü taşıyor mu.
/// (server route grant öncesi çağırır; `user_role::check_user_role`'ün ince sarmalı.)
pub async fn delegator_holds_seat(
    pool: &PgPool,
    delegator_user_id: Uuid,
    seat_orgu_id: Uuid,
    seat_role: &str,
) -> Result<bool, OrgError> {
    super::user_role::check_user_role(pool, delegator_user_id, seat_orgu_id, seat_role).await
}
