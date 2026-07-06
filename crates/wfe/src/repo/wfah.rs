use crate::{error::WfeError, models::WfahRow};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<WfahRow>, WfeError> {
    sqlx::query_as::<_, WfahRow>(
        "SELECT wfah_id, wfe_id, seq, action, actor, input, applied_at
         FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq ASC",
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
