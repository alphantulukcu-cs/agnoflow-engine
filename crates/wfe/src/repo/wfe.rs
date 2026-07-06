use crate::{error::WfeError, models::WfeRow};
use sqlx::PgPool;
use uuid::Uuid;

const WFE_COLUMNS: &str = "wfe_id, orgtnt_id, wfd_id, wfd_version, status,
                current_node, current_c_a, claimed_by, end_response, created_at, updated_at";

pub async fn get(pool: &PgPool, wfe_id: Uuid) -> Result<WfeRow, WfeError> {
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe WHERE wfe_id = $1"
    ))
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(wfe_id.to_string()))
}

pub async fn list_by_tenant(pool: &PgPool, orgtnt_id: Uuid) -> Result<Vec<WfeRow>, WfeError> {
    sqlx::query_as::<_, WfeRow>(&format!(
        "SELECT {WFE_COLUMNS} FROM wf.wfe WHERE orgtnt_id = $1 ORDER BY created_at DESC"
    ))
    .bind(orgtnt_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

/// Escalation / root-timeout süpürücüsü için tüm aktif WFE id'leri.
pub async fn list_active_ids(pool: &PgPool) -> Result<Vec<Uuid>, WfeError> {
    sqlx::query_scalar::<_, Uuid>("SELECT wfe_id FROM wf.wfe WHERE status = 'active'")
        .fetch_all(pool)
        .await
        .map_err(WfeError::Database)
}
