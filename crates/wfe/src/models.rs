use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, FromRow, serde::Serialize)]
pub struct WfeRow {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    /// Koşum ortamı ($env) — start'ta sabitlenir, ömür boyu değişmez.
    /// NULL = tenant'ın varsayılan ortamı (kolon NOT NULL'a çekilene kadarki geçiş).
    pub environment_id: Option<Uuid>,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub status: String,
    /// v2.2: aktif WFE'nin beklediği node slug'ı (WOR-24).
    ///
    /// SERİLEŞMEZ: dışarı çıkan hâli `{id, label}` çiftidir (`executor::Ref`) ve
    /// etiket ancak WFD elde varken üretilebilir — o da satırın değil, listeleyen
    /// rotanın (wfd cache'i orada) işidir. Ham anahtarı ayrıca yollamak istemciye
    /// "hangisini basayım" sorusunu geri verirdi.
    #[serde(skip_serializing)]
    pub current_node: Option<String>,
    pub current_c_a: serde_json::Value,
    /// Görünürlük projeksiyonu (2026-08-13): `listable ∪ wf_admin` grant'ları.
    /// `current_c_a`dan farkı: terminal'de SİLİNMEZ ve `when` uygulanmıştır.
    /// SERİLEŞMEZ — istemcinin işi değil, SQL süzgecinin dayanağıdır.
    #[serde(skip_serializing)]
    pub view_c_a: serde_json::Value,
    /// `listable`/`wf_admin` ORGTRVLANG çapası (akışı başlatanın birimi).
    /// NULL = backfill bekleyen eski satır (bkz. `Wfes::origin_orgu_id`).
    #[serde(skip_serializing)]
    pub origin_orgu_id: Option<Uuid>,
    pub claimed_by: Option<serde_json::Value>,
    pub end_response: Option<serde_json::Value>,
    /// SLA-3: çözülmüş mutlak workflow deadline'ı; NULL = yok (2026-07-16).
    pub deadline: Option<DateTime<Utc>>,
    /// SLA-1: en son claim anı; claimed_by temizlenince NULL'lanır (node değişimi dahil).
    pub claimed_at: Option<DateTime<Utc>>,
    /// WOR-31: fork'ta persist edilen join hedefi ({node}/{terminal} untagged
    /// JSON); NOT NULL = paralel mod (bu durumda current_node NULL'dır).
    pub join_target: Option<serde_json::Value>,
    /// WOR-72: fork'ta persist edilen quorum eşiği; NULL = AND-join (tüm kollar
    /// beklenir), k = k varış yeterli (kalan kollar eşik dolunca cancelled).
    pub join_threshold: Option<i32>,
    /// WOR-73: fork'ta persist edilen ZEN join koşulu (`join_mode: expr`); NULL =
    /// eşik/AND kuralı. `join_threshold` ile ikisi birden dolu OLAMAZ (DB CHECK).
    pub join_when: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// WOR-31: paralel mod kol satırı (`wf.wfe_branch`).
#[derive(Debug, FromRow)]
pub struct BranchRow {
    /// TEK WFE okunurken de seçilir: aynı satır tipi TOPLU sorguda da kullanılır
    /// (`repo::branch::load_all_for_wfes`) ve orada gruplama anahtarıdır.
    pub wfe_id: Uuid,
    pub branch_node: String,
    /// WOR-73: kolun değişmez kimliği (fork'taki giriş node'u). WOR-73 öncesi
    /// satırlarda NULL olabilir → çağıran `branch_node`'a düşer.
    pub entry_node: Option<String>,
    pub status: String,
    pub claimed_by: Option<serde_json::Value>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub entered_at: DateTime<Utc>,
}

/// WOR-31 T4: liste görünümü için `wfe_id` taşıyan kol satırı — `GET /wfe` birden
/// çok WFE'nin kollarını TEK sorguda çekip `wfe_id`'ye göre gruplar (satır-başına
/// sorgu YOK, bkz. `repo::branch::load_active_for_wfes`).
#[derive(Debug, FromRow)]
pub struct BranchListRow {
    pub wfe_id: Uuid,
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
    /// K7 (Faz 0, 2026-08-10): bu WFAH satırının geçişten ÖNCEki node'u.
    /// NULL = start (öncesi yok) veya sütun eklenmeden önceki eski satır.
    pub from_node: Option<String>,
    /// K7: geçişin hedef node'u. NULL = terminal/failed/terminated veya
    /// çok-hedefli fork (hedefler `wf.wfe_branch`'te satır satır durur).
    pub to_node: Option<String>,
}
