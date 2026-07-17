//! WOR-31: paralel mod kol satırları (`wf.wfe_branch`) okuma yardımcıları.
//! Yazımlar (insert/CAS/cancel) `wfe_adapter.rs` içinde commit transaction'ına
//! gömülüdür — atomiklik için tek tx'te kalmaları gerekir.

use crate::{error::WfeError, models::BranchRow};
use sqlx::PgPool;
use uuid::Uuid;

/// WFE'nin TÜM kol satırları (active + arrived + cancelled). Engine `active`
/// olanları kendi filtreler; cancelled/arrived yükü audit + join doğrulaması
/// için taşınır. entered_at'a göre sıralı — deterministik görünüm.
pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<BranchRow>, WfeError> {
    sqlx::query_as::<_, BranchRow>(
        "SELECT branch_node, status, claimed_by, claimed_at, entered_at
         FROM wf.wfe_branch WHERE wfe_id = $1 ORDER BY entered_at, branch_node",
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
