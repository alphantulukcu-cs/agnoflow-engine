//! WOR-31: paralel mod kol satırları (`wf.wfe_branch`) okuma yardımcıları.
//! Yazımlar (insert/CAS/cancel) `wfe_adapter.rs` içinde commit transaction'ına
//! gömülüdür — atomiklik için tek tx'te kalmaları gerekir.

use crate::{
    error::WfeError,
    models::{BranchListRow, BranchRow},
};
use sqlx::PgPool;
use uuid::Uuid;

/// WFE'nin TÜM kol satırları (active + arrived + cancelled). Engine `active`
/// olanları kendi filtreler; cancelled/arrived yükü audit + join doğrulaması
/// için taşınır. entered_at'a göre sıralı — deterministik görünüm.
pub async fn load_all(pool: &PgPool, wfe_id: Uuid) -> Result<Vec<BranchRow>, WfeError> {
    sqlx::query_as::<_, BranchRow>(
        "SELECT wfe_id, branch_node, entry_node, status, claimed_by, claimed_at, entered_at
         FROM wf.wfe_branch WHERE wfe_id = $1 ORDER BY entered_at, branch_node",
    )
    .bind(wfe_id)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

/// `load_all`in TOPLU hâli — verilen WFE'lerin TÜM kolları, TEK sorguda.
///
/// `load_active_for_wfes`ten farkı bilinçlidir: o havuz/liste GÖRÜNÜMÜ içindir ve
/// yalnız aktif kolları taşır; bu ise `Wfes` KURMAK içindir, dolayısıyla `load_all`
/// ile aynı kümeyi (active + arrived + cancelled) ve aynı sırayı döndürmek
/// ZORUNDADIR — `WfeStore::load_many` sözleşmesi "load ile tıpatıp aynı" der.
/// Sıralama `wfe_id` ile başlar ki gruplama tek geçişte yapılabilsin.
pub async fn load_all_for_wfes(
    pool: &PgPool,
    wfe_ids: &[Uuid],
) -> Result<Vec<BranchRow>, WfeError> {
    if wfe_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, BranchRow>(
        "SELECT wfe_id, branch_node, entry_node, status, claimed_by, claimed_at, entered_at
         FROM wf.wfe_branch WHERE wfe_id = ANY($1) ORDER BY wfe_id, entered_at, branch_node",
    )
    .bind(wfe_ids)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}

/// WOR-31 T4: verilen WFE'lerin AKTİF kolları — TEK sorgu (liste fan-out'u için;
/// çağıran `wfe_id`'ye göre gruplar). Yalnız `active`: pool listesi claim
/// edilebilir işleri gösterir, arrived/cancelled kollar audit'e ait (detay
/// görünümü `load_all` ile hepsini taşır). entered_at/node'a göre sıralı.
pub async fn load_active_for_wfes(
    pool: &PgPool,
    wfe_ids: &[Uuid],
) -> Result<Vec<BranchListRow>, WfeError> {
    if wfe_ids.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as::<_, BranchListRow>(
        "SELECT wfe_id, branch_node, status, claimed_by, claimed_at, entered_at
         FROM wf.wfe_branch
         WHERE wfe_id = ANY($1) AND status = 'active'
         ORDER BY entered_at, branch_node",
    )
    .bind(wfe_ids)
    .fetch_all(pool)
    .await
    .map_err(WfeError::Database)
}
