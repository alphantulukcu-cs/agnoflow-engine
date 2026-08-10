use crate::{error::WfeError, models::WfahRow};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// WOR-65: verilen WFE'ler için revizyon token'ları — `max(seq)` per WFE
/// (bkz. `Wfes::rev()`). LİSTE endpoint'leri içindir: tek turda tüm satırların
/// revizyonunu getirir, N+1 sorgu üretmez. Hiç WFAH kaydı olmayan WFE haritada
/// YER ALMAZ (çağıran 0'a düşer) — pratikte olmaz, `create` daima kayıt yazar.
pub async fn max_seq_by_wfe(
    pool: &PgPool,
    wfe_ids: &[Uuid],
) -> Result<HashMap<Uuid, i32>, WfeError> {
    if wfe_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query_as::<_, (Uuid, i32)>(
        "SELECT wfe_id, max(seq) FROM wf.wfah WHERE wfe_id = ANY($1) GROUP BY wfe_id",
    )
    .bind(wfe_ids)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)?;
    Ok(rows.into_iter().collect())
}

pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<WfahRow>, WfeError> {
    sqlx::query_as::<_, WfahRow>(
        "SELECT wfah_id, wfe_id, seq, action, actor, input, applied_at, from_node, to_node
         FROM wf.wfah WHERE wfe_id = $1 ORDER BY seq ASC",
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
