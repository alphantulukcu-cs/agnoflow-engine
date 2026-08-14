use crate::error::WfeError;
use sqlx::PgPool;
use uuid::Uuid;

/// Returns the latest DynCtx snapshot for a WFE.
pub async fn load_latest(pool: &PgPool, wfe_id: Uuid) -> Result<serde_json::Value, WfeError> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT ctx FROM wf.wfe_dynctx
         WHERE wfe_id = $1 ORDER BY seq DESC LIMIT 1",
    )
    .bind(wfe_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| WfeError::NotFound(format!("dynctx for wfe {wfe_id}")))
}

/// `load_latest`in TOPLU hâli — WFE başına EN SON snapshot, TEK sorguda
/// (`WfeStore::load_many` için). `DISTINCT ON` + `seq DESC` tek-WFE sorgusunun
/// `ORDER BY seq DESC LIMIT 1`i ile aynı satırı seçer. Snapshot'ı olmayan WFE
/// sonuçta YER ALMAZ (tek-WFE yolunda `NotFound`un karşılığı).
pub async fn load_latest_many(
    pool: &PgPool,
    wfe_ids: &[Uuid],
) -> Result<Vec<(Uuid, serde_json::Value)>, WfeError> {
    if wfe_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, (Uuid, serde_json::Value)>(
        "SELECT DISTINCT ON (wfe_id) wfe_id, ctx FROM wf.wfe_dynctx
         WHERE wfe_id = ANY($1) ORDER BY wfe_id, seq DESC",
    )
    .bind(wfe_ids)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
