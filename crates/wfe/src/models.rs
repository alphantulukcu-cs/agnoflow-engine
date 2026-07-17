use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow, serde::Serialize)]
pub struct WfeRow {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub status: String,
    /// v2.2: aktif WFE'nin beklediği node slug'ı (WOR-24).
    pub current_node: Option<String>,
    pub current_c_a: serde_json::Value,
    pub claimed_by: Option<serde_json::Value>,
    pub end_response: Option<serde_json::Value>,
    /// SLA-3: çözülmüş mutlak workflow deadline'ı; NULL = yok (2026-07-16).
    pub deadline: Option<DateTime<Utc>>,
    /// SLA-1: en son claim anı; claimed_by temizlenince NULL'lanır (node değişimi dahil).
    pub claimed_at: Option<DateTime<Utc>>,
    /// WOR-31: fork'ta persist edilen AND-join hedefi ({node}/{terminal} untagged
    /// JSON); NOT NULL = paralel mod (bu durumda current_node NULL'dır).
    pub join_target: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// WOR-31: paralel mod kol satırı (`wf.wfe_branch`).
#[derive(Debug, FromRow)]
pub struct BranchRow {
    pub branch_node: String,
    pub status: String,
    pub claimed_by: Option<serde_json::Value>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub entered_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct WfahRow {
    pub wfah_id: Uuid,
    pub wfe_id: Uuid,
    pub seq: i32,
    pub action: String,
    pub actor: serde_json::Value,
    pub input: Option<serde_json::Value>,
    pub applied_at: DateTime<Utc>,
}
