use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow, serde::Serialize)]
pub struct WfeRow {
    pub wfe_id:       Uuid,
    pub orgtnt_id:    Uuid,
    pub wfd_id:       Uuid,
    pub wfd_version:  i32,
    pub status:       String,
    pub current_c_a:  serde_json::Value,
    pub end_response: Option<serde_json::Value>,
    pub created_at:   DateTime<Utc>,
    pub updated_at:   DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct DynCtxRow {
    pub dynctx_id:  Uuid,
    pub wfe_id:     Uuid,
    pub seq:        i32,
    pub ctx:        serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct WfahRow {
    pub wfah_id:    Uuid,
    pub wfe_id:     Uuid,
    pub seq:        i32,
    pub action:     String,
    pub actor:      serde_json::Value,
    pub input:      Option<serde_json::Value>,
    pub applied_at: DateTime<Utc>,
}
