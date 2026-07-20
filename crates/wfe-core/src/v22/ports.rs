//! v2.2 runtime port'ları ve commit modeli.
//! Engine saf hesap yapar; persistence TransitionCommit ile TEK transaction'da
//! store'a devredilir (M8 / WOR-43 — atomik pipeline).

use crate::error::EngineError;
use crate::types::actor::{Actor, CandidateActor as ResolvedCandidate};
use crate::types::dynctx::DynCtx;
use crate::types::wfah::{Wfah, WfahEntry};
use crate::types::wfd_v22::{AutoexecDef, Wfd, WftTarget};
use crate::types::wfe::WfeStatus;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// WOR-31: paralel mod kol durumu. `Serialize`/`Deserialize` — T4: API görünümü
/// (`GET /wfe/:id`, sim step response) ve `SimState` bu tipi doğrudan taşır.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BranchStatus {
    Active,
    Arrived,
    Cancelled,
}

/// WOR-31: paralel modda tek kolun runtime durumu — claim/entered alanları
/// KOL-bazlıdır (escalation ve claim_timeout paralel modda kol üzerinden işler).
/// `Serialize`/`Deserialize` (T4): API/sim görünümünde JSON alan adı `node`'dur
/// (Rust tarafında `branch_node` kalır — motor içi isimlendirme değişmedi).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchState {
    /// Kol token'ının şu an beklediği node slug'ı (fork'ta kolun giriş node'u).
    #[serde(rename = "node")]
    pub branch_node: String,
    pub status: BranchStatus,
    pub claimed_by: Option<Uuid>,
    pub claimed_at: Option<DateTime<Utc>>,
    /// Kolun bu node'a giriş anı — kol escalation dwell'i buradan ölçülür.
    pub entered_at: DateTime<Utc>,
}

/// v2.2 WFES — current_node + assignment (WOR-24).
#[derive(Debug, Clone)]
pub struct Wfes {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub dynctx: DynCtx,
    pub wfah: Wfah,
    pub status: WfeStatus,
    /// Aktif WFE'nin beklediği node slug'ı; terminal'de None.
    pub current_node: Option<String>,
    /// Claim eden kullanıcı (assignment). Node değişiminde temizlenir (M8).
    pub assigned_to: Option<Uuid>,
    pub end_response: Option<Value>,
    /// SLA-3: çözülmüş mutlak workflow deadline'ı (start'ta hesaplanır); NULL = yok.
    pub deadline: Option<DateTime<Utc>>,
    /// SLA-1: en son claim anı — claimed_by temizlenince NULL'lanır (node değişimi dahil).
    pub claimed_at: Option<DateTime<Utc>>,
    /// Priority hesabı (elapsed/window) için pencere başlangıcı.
    pub created_at: DateTime<Utc>,
    /// WOR-31: paralel mod kol durumları — paralel modda değilken boş.
    pub branches: Vec<BranchState>,
    /// WOR-31: fork'ta persist edilen AND-join hedefi; `Some(..)` = paralel mod
    /// (bu durumda `current_node` NULL'dır).
    pub join_target: Option<WftTarget>,
}

/// Transition sonucunun gideceği yer.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitOutcome {
    MoveTo { node: String },
    Terminal { end_response: Value },
    /// Engine-defined fail (§5 root timeout vb.) — WFE `error` durumuna alınır,
    /// başarılı `Terminal` sonlanmasından ayrıdır.
    Failed { end_response: Value },
    /// SLA ihlali (deadline aşımı / dwell terminate) — WFE `terminated` durumuna
    /// alınır (2026-07-16). Hata değil, başarılı `Terminal` da değil.
    Terminated { end_response: Value },
    /// WOR-31 fork: her kol için branch satırı yaratılır, `current_node = NULL`,
    /// `join_target` persist edilir (paralel moda giriş).
    ForkTo {
        branches: Vec<String>,
        join: WftTarget,
    },
    /// WOR-31: tek kolun token hareketi — kol claim'i + entered_at sıfırlanır,
    /// paralel mod sürer.
    BranchMoveTo { from_node: String, node: String },
    /// WOR-31: kol join hedefine vardı, engine'in görüşüne göre ≥1 başka aktif
    /// kol kaldı — kol `arrived` işaretlenir (join node'u İŞGAL ETMEZ).
    BranchArrived { from_node: String },
    /// WOR-31: varan kol engine'in görüşüne göre SONUNCU — paralel mod biter;
    /// `next` join hedefi: `MoveTo{join}` veya join terminal ise `Terminal{..}`.
    /// Yarış adapter doğrulaması + executor retry ile çözülür (T3); engine saftır.
    JoinComplete {
        from_node: String,
        next: Box<CommitOutcome>,
    },
    /// WOR-56: kol collapse aksiyonu bir NODE hedefine — paralel mod biter,
    /// diğer TÜM aktif kollar `cancelled` (engine `_branch_cancelled` marker'ları
    /// stage eder), `current_node = node`. Kol-arrival sayımı YOK: collapse
    /// otoriterdir (JoinComplete'in aksine kalan aktif kolları beklemez).
    /// (Terminal hedefli collapse ayrı bir varyant değildir — mevcut `Terminal`
    /// yolu zaten paralel modda kardeşleri iptal eder.)
    ///
    /// WOR-62: "otoriter" ≠ "serileştirilmemiş". Store commit'i tx başında
    /// `FOR UPDATE` alıp kilit altında hâlâ paralel modda olduğunu doğrular;
    /// bir kardeş önce davrandıysa bu collapse `Conflict(ConflictKind::Collapsed)`
    /// ile kaybeder. Kural: ilk kilidi alan kazanır.
    CollapseTo {
        from_node: String,
        node: String,
    },
}

/// Tek transaction'da persist edilecek transition sonucu (M8).
/// Unhandled fail durumunda hiçbir parçası yazılmaz.
#[derive(Debug, Clone)]
pub struct TransitionCommit {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub new_dynctx: Value,
    pub wfah_entries: Vec<WfahEntry>,
    pub outcome: CommitOutcome,
    /// Yeni node'un resolve edilmiş aday listesi — pool sorguları için denormalize cache.
    pub resolved_c_a: Vec<ResolvedCandidate>,
}

/// Yeni WFE oluşturma isteği — wfe_id ENGINE tarafından üretilir ve effects
/// gerçek id ile çözülür (WOR-6 fix: temp UUID yok).
#[derive(Debug, Clone)]
pub struct NewWfe {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub wfd_id: Uuid,
    pub wfd_version: i32,
    pub initial_dynctx: Value,
    pub wfah_entries: Vec<WfahEntry>,
    pub outcome: CommitOutcome,
    pub resolved_c_a: Vec<ResolvedCandidate>,
    /// SLA-3: start'ta çözülen mutlak deadline (bkz. `Engine::start`); NULL = yok.
    pub deadline: Option<DateTime<Utc>>,
}

#[async_trait]
pub trait WfdStore: Send + Sync {
    /// v2.2 yükleme kapısından geçmiş WFD döner (M14).
    async fn fetch(&self, wfd_id: Uuid, version: i32) -> Result<Wfd, EngineError>;
}

#[async_trait]
pub trait WfeStore: Send + Sync {
    async fn load(&self, wfe_id: Uuid) -> Result<Wfes, EngineError>;
    /// Yeni WFE'yi tüm başlangıç durumu ile TEK transaction'da yaratır.
    async fn create(&self, new: &NewWfe) -> Result<(), EngineError>;
    /// Transition sonucunu TEK transaction'da uygular:
    /// dynctx snapshot + WFAH append + node/terminal + assignment reset.
    async fn commit(&self, commit: &TransitionCommit) -> Result<(), EngineError>;
    /// Atomik claim (CAS): yalnızca unassigned ise yazar; başarıyı döner.
    /// `branch`: WOR-31 — paralel modda kol node'u verilir; CAS o kolun
    /// `wf.wfe_branch` satırında yapılır (status='active' AND claimed_by IS NULL);
    /// `None` paralel-olmayan wfe-seviyesi claim (mevcut davranış).
    async fn claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        user_id: Uuid,
        branch: Option<&str>,
    ) -> Result<bool, EngineError>;
    /// SLA-1 claim timeout (wft'siz kol): node DEĞİŞMEDEN claimed_by/claimed_at
    /// temizlenir + WFAH marker eklenir — `commit()`'ten ayrı çünkü node/status
    /// değişmiyor (bkz. `Engine::fire_claim_timeout` / `ClaimTimeoutOutcome::Release`).
    /// `branch`: WOR-31 — paralel modda o kolun claim'i sıfırlanır.
    async fn release_claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
    ) -> Result<(), EngineError>;
}

/// Autoexec çalıştırma hatası — WFD.* hata taksonomisi (M9).
#[derive(Debug, Clone)]
pub struct ExecFailure {
    /// "WFD.Timeout", "WFD.AutoexecFailed" gibi hata adı.
    pub error: String,
    pub message: String,
}

impl ExecFailure {
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            error: "WFD.AutoexecFailed".into(),
            message: message.into(),
        }
    }
    pub fn timeout() -> Self {
        Self {
            error: "WFD.Timeout".into(),
            message: "autoexec timeout_seconds aşıldı".into(),
        }
    }
}

/// Autoexec çalıştırma bağlamı — config içindeki $ctx.* parametreleri
/// runner tarafında bu ctx ile çözülür.
#[derive(Debug, Clone)]
pub struct ExecEnv {
    pub wfe_id: Uuid,
    pub ctx: Value,
    pub node: Option<String>,
    pub actor: Actor,
}

#[async_trait]
pub trait AutoexecRunner: Send + Sync {
    /// Ham sonucu döner; timeout PIPELINE tarafından uygulanır.
    async fn run(&self, def: &AutoexecDef, env: &ExecEnv) -> Result<Value, ExecFailure>;
}
