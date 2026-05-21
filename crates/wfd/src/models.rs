use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct WfdMeta {
    pub wfd_id:     Uuid,
    pub orgtnt_id:  Uuid,
    pub name:       String,
    pub version:    i32,
    pub s3_key:     String,
    pub is_active:  bool,
    pub created_at: DateTime<Utc>,
}
