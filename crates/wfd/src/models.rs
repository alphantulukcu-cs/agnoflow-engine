use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct Project {
    pub project_id:  Uuid,
    pub orgtnt_id:   Uuid,
    pub name:        String,
    pub description: Option<String>,
    pub created_at:  DateTime<Utc>,
    pub updated_at:  DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct WfdMeta {
    pub wfd_id:      Uuid,
    pub orgtnt_id:   Uuid,
    pub project_id:  Uuid,
    pub name:        String,
    pub version:     i32,
    pub s3_key:      String,
    pub is_active:   bool,
    pub created_at:  DateTime<Utc>,
    pub status:      String,
    pub description: Option<String>,
    pub tags:        Vec<String>,
    pub owner:       String,
    pub updated_at:  DateTime<Utc>,
    /// Türetildiği predefined şablon versiyonu (varsa).
    pub source_template_id: Option<Uuid>,
    /// Son ret gerekçesi (reject sonrası draft'ta gösterilir).
    pub review_note: Option<String>,
    /// Onaya gönderen kullanıcı (e-posta) — pending satırda dolu.
    pub submitted_by: Option<String>,
}
