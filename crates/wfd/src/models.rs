use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Serialize, FromRow)]
pub struct Project {
    pub project_id: Uuid,
    pub orgtnt_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct WfdMeta {
    pub wfd_id: Uuid,
    pub orgtnt_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub version: i32,
    pub s3_key: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub status: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub owner: String,
    pub updated_at: DateTime<Utc>,
    /// Türetildiği predefined şablon versiyonu (varsa).
    pub source_template_id: Option<Uuid>,
    /// Son ret gerekçesi (reject sonrası draft'ta gösterilir).
    pub review_note: Option<String>,
    /// Onaya gönderen kullanıcı (e-posta) — pending satırda dolu.
    pub submitted_by: Option<String>,
    /// WFC: dokümanın kendi `id` alanı. `calls.<key>.wfd_id` BUNA atıfta bulunur
    /// (DB uuid'sine değil) — editörün akış seçici dropdown'ı bu değeri yazar.
    /// Migration öncesi satırlarda NULL; yeniden yayınlanınca dolar.
    pub doc_id: Option<String>,
    /// WFC: dokümanın semver `version`'ı — `calls.<key>.version` pinlemesi için.
    pub doc_version: Option<String>,
    /// T‑B4 taslak kilidi: kilidi tutan kullanıcı. `NULL` = serbest.
    pub lock_user_id: Option<Uuid>,
    /// Kilidin alındığı an — "bu kişi bu taslağı ne zamandır tutuyor". Kilidin süresi
    /// YOKTUR; sahiplik bırakılana kadar sürer.
    pub lock_acquired_at: Option<DateTime<Utc>>,
}
