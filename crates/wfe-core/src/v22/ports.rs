//! v2.2 runtime port'ları ve commit modeli.
//! Engine saf hesap yapar; persistence TransitionCommit ile TEK transaction'da
//! store'a devredilir (M8 / WOR-43 — atomik pipeline).

use crate::error::EngineError;
use crate::types::actor::{Actor, CandidateActor as ResolvedCandidate};
use crate::types::dynctx::DynCtx;
use crate::types::wfah::{Wfah, WfahEntry};
use crate::types::wfd_v22::{AutoexecDef, CallMode, StartAs, Wfd, WftTarget};
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

impl Wfes {
    /// WOR-65: WFE **revizyon token'ı** = son WFAH kaydının `seq`'i.
    ///
    /// Neden yeni bir kolon değil: `wf.wfah` zaten WFE başına monotonik bir sayaç
    /// tutar (`UNIQUE (wfe_id, seq)`), her transition en az bir kayıt ekler ve
    /// `commit` tek transaction'dır — yani "revizyon" bilgisi zaten kalıcı,
    /// atomik ve çakışmaya karşı DB kısıtıyla korunuyor. Ayrı bir `rev` kolonu
    /// aynı gerçeği ikinci kez saklayıp senkron tutma yükü getirirdi.
    ///
    /// **Kapsam istisnası (bilinçli):** `WfeStore::claim` WFAH'a yazmaz (saf CAS
    /// UPDATE'tir), dolayısıyla claim revizyonu ARTIRMAZ. Claim yarışları kendi
    /// `claimed_by IS NULL` CAS'ıyla korunur. `release_claim` ise WFAH marker'ı
    /// yazdığı için revizyonu artırır. Ayrıntı: `docs/spec/decisions.md`.
    ///
    /// WFAH boşsa 0 — pratikte olmaz (`Engine::start` daima ≥1 kayıt stage eder),
    /// ama 0 "hiç revizyon yok" olarak güvenli biçimde okunur.
    pub fn rev(&self) -> u32 {
        self.wfah.entries().last().map(|e| e.seq).unwrap_or(0)
    }
}

/// Transition sonucunun gideceği yer.
#[derive(Debug, Clone, PartialEq)]
pub enum CommitOutcome {
    MoveTo {
        node: String,
    },
    Terminal {
        end_response: Value,
    },
    /// Engine-defined fail (§5 root timeout vb.) — WFE `error` durumuna alınır,
    /// başarılı `Terminal` sonlanmasından ayrıdır.
    Failed {
        end_response: Value,
    },
    /// SLA ihlali (deadline aşımı / dwell terminate) — WFE `terminated` durumuna
    /// alınır (2026-07-16). Hata değil, başarılı `Terminal` da değil.
    Terminated {
        end_response: Value,
    },
    /// WOR-31 fork: her kol için branch satırı yaratılır, `current_node = NULL`,
    /// `join_target` persist edilir (paralel moda giriş).
    ForkTo {
        branches: Vec<String>,
        join: WftTarget,
    },
    /// WOR-31: tek kolun token hareketi — kol claim'i + entered_at sıfırlanır,
    /// paralel mod sürer.
    BranchMoveTo {
        from_node: String,
        node: String,
    },
    /// WOR-31: kol join hedefine vardı, engine'in görüşüne göre ≥1 başka aktif
    /// kol kaldı — kol `arrived` işaretlenir (join node'u İŞGAL ETMEZ).
    BranchArrived {
        from_node: String,
    },
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

/// WFC çağrısının yapıldığı yer. Mod ile birlikte "nasıl çağrıldı"yı tamamlar.
#[derive(Debug, Clone, PartialEq)]
pub enum CallSite {
    /// Alt akış çağrısı — çağrı node'unun slug'ı (`mode: wait | detached`).
    Node(String),
    /// Ardıl akış — terminal id'si (`mode: terminal`).
    Terminal(String),
}

impl CallSite {
    pub fn kind(&self) -> &'static str {
        match self {
            CallSite::Node(_) => "node",
            CallSite::Terminal(_) => "terminal",
        }
    }
    pub fn key(&self) -> &str {
        match self {
            CallSite::Node(k) | CallSite::Terminal(k) => k,
        }
    }
}

/// Commit ile AYNI transaction'da kuyruğa alınacak WFC çağrısı (outbox satırı).
///
/// Neden outbox: çağrılan WFE'yi commit'in İÇİNDE yaratmak, çağıranın atomik
/// transaction'ını başka bir WFE'nin tüm start pipeline'ına (org resolve, trigger'lar,
/// kendi commit'i) bağlardı. Bunun yerine niyet aynı tx'te kalıcı hale getirilir,
/// gerçek start ayrı bir tx'te koşar — böylece çağıranın atomikliği bozulmaz ve
/// başlatma yeniden denenebilir olur.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedCall {
    /// Root `calls` katalogundaki key.
    pub call_key: String,
    pub mode: CallMode,
    pub site: CallSite,
    /// WFC-IN — çağıranın ctx'ine göre ÇÖZÜLMÜŞ girdi (çağrılanın start ACT input'u).
    pub input: Value,
    /// `wait` için mutlak son tarih (`call.timeout` çözümü). Yok = sınırsız.
    pub deadline: Option<DateTime<Utc>>,
    /// Yalnız `terminal`: ardılı kim başlatır.
    pub start_as: StartAs,
    /// Yalnız `terminal`: ardıl döngüsü için yerel üst sınır.
    pub max_next: Option<u32>,
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
    /// WFC outbox — bu commit ile aynı tx'te `queued` olarak yazılır.
    pub staged_calls: Vec<StagedCall>,
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
    /// WFC outbox — start pipeline'ında stage edilen çağrılar (start kuralının wft'si
    /// doğrudan bir çağrı node'una gidebilir).
    pub staged_calls: Vec<StagedCall>,
    /// Bu WFE bir WFC ile yaratıldıysa çağıran bağlantısı; kök WFE'de `None`.
    pub caller: Option<CallLink>,
}

/// Çağıran ↔ çağrılan bağı. `depth`/`next_depth` çağıranın satırından +1 taşınır —
/// yuvalanma ve ardıl zinciri AYRI sayılır çünkü frenleri de ayrıdır.
#[derive(Debug, Clone, PartialEq)]
pub struct CallLink {
    /// `wf.wfe_call` satırının id'si — start başarılıysa `running`'e çekilir.
    pub call_row_id: Uuid,
    pub caller_wfe_id: Uuid,
    pub site: CallSite,
    pub call_key: String,
    pub mode: CallMode,
    /// Alt akış yuvalanma derinliği (`wait`/`detached` ile artar).
    pub depth: i32,
    /// Ardıl zinciri uzunluğu (`terminal` ile artar).
    pub next_depth: i32,
}

/// Bir WFC satırının API görünümü. `PendingCall`'dan ayrıdır: burada yalnız
/// istemcinin göreceği alanlar var (girdi/derinlik gibi iç detaylar YOK).
#[derive(Debug, Clone, serde::Serialize)]
pub struct CallView {
    /// 'node' (alt akış) | 'terminal' (ardıl akış).
    pub site_kind: String,
    /// Çağrı node'unun slug'ı ya da terminal id'si.
    pub site_key: String,
    pub call_key: String,
    /// wait | detached | terminal
    pub mode: String,
    /// queued | running | returned | consumed | failed | cancelled | skipped
    pub status: String,
    /// `caller_of` görünümünde çağıranın id'si; `calls_of_caller`'da çağrılanın id'si.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wfe_id: Option<Uuid>,
    /// completed | failed | terminated | timeout — çağrılan bittiyse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_status: Option<String>,
}

/// Bekleyen bir WFC satırı — executor'ın outbox/dönüş taramaları bunu okur.
#[derive(Debug, Clone)]
pub struct PendingCall {
    pub id: Uuid,
    pub orgtnt_id: Uuid,
    pub caller_wfe_id: Uuid,
    pub site: CallSite,
    pub call_key: String,
    pub mode: CallMode,
    pub input: Value,
    pub deadline: Option<DateTime<Utc>>,
    pub start_as: StartAs,
    pub max_next: Option<u32>,
    pub depth: i32,
    pub next_depth: i32,
    /// Dönüş taramasında dolu: çağrılanın kimliği + sonucu.
    pub callee_wfe_id: Option<Uuid>,
    pub end_response: Option<Value>,
    /// "completed" | "failed" | "terminated" | "timeout"
    pub call_status: Option<String>,
}

#[async_trait]
pub trait WfdStore: Send + Sync {
    /// v2.2 yükleme kapısından geçmiş WFD döner (M14).
    async fn fetch(&self, wfd_id: Uuid, version: i32) -> Result<Wfd, EngineError>;

    /// WFC: dokümanın `id` alanından (ve opsiyonel semver'inden) yayınlanmış satırı çözer.
    ///
    /// `calls.<key>.wfd_id` bir DB uuid'si değil, çağrılan WFD'nin doküman kimliğidir —
    /// bu yüzden ayrı bir çözüm adımı gerekir. `doc_version: None` = en son yayınlanmış.
    /// Varsayılan `None` döner: WFC'yi desteklemeyen store'larda çağrı başlatma
    /// `WFD.CallNotFound` ile başarısız olur (sessizce yanlış WFD çalıştırmaktansa).
    async fn resolve_doc(
        &self,
        orgtnt_id: Uuid,
        doc_id: &str,
        doc_version: Option<&str>,
    ) -> Result<Option<(Uuid, i32)>, EngineError> {
        let (_, _, _) = (orgtnt_id, doc_id, doc_version);
        Ok(None)
    }
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
    /// `marker`: Madde 6 — vekaleten claim'de CAS kazanılırsa AYNI transaction'da
    /// yazılacak WFAH audit kaydı (`claim:delegated`). Doğrudan claim'de `None`.
    async fn claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        user_id: Uuid,
        branch: Option<&str>,
        marker: Option<&WfahEntry>,
    ) -> Result<bool, EngineError>;
    /// SLA-1 claim timeout (wft'siz kol): node DEĞİŞMEDEN claimed_by/claimed_at
    /// temizlenir + WFAH marker eklenir — `commit()`'ten ayrı çünkü node/status
    /// değişmiyor (bkz. `Engine::fire_claim_timeout` / `ClaimTimeoutOutcome::Release`).
    /// `branch`: WOR-31 — paralel modda o kolun claim'i sıfırlanır.
    /// `new_dynctx`: 2026-07-28 — SLA-1 `wfes_effects` uygulanmışsa yeni ctx AYNI
    /// transaction'da `wf.wfe_dynctx`'e yazılır; `None` ise ctx'e dokunulmaz.
    async fn release_claim(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
        new_dynctx: Option<&Value>,
    ) -> Result<(), EngineError>;
    /// Madde 7: yetkili devir. `claim`'in CAS'ının aksine zaten sahipli (ya da
    /// havuzdaki) bir satırı override eder — uygunluk `Engine::reassign`'da
    /// (reassign c_a + hedef node c_a) doğrulanmıştır. `target = Some` belirli
    /// kullanıcıya devir (claimed_at = now()), `None` havuza bırakma (claimed_by/
    /// claimed_at NULL). WFAH marker + assignment TEK transaction'da yazılır.
    /// `branch`: WOR-31 — paralel modda yalnız o kolun sahipliği değişir.
    async fn reassign(
        &self,
        wfe_id: Uuid,
        orgtnt_id: Uuid,
        target: Option<Uuid>,
        wfah_entry: &WfahEntry,
        branch: Option<&str>,
    ) -> Result<(), EngineError>;

    // ---- WFC (iş akışı çağrısı) ----
    //
    // Varsayılan implementasyonlar WFC'yi DESTEKLEMEYEN store'lar için "hiç çağrı yok"
    // davranışı verir: mevcut test store'ları (ve WFC kullanmayan kurulumlar) değişmeden
    // derlenir, executor'ın tarama döngüleri de boş küme görüp hiçbir şey yapmaz.

    /// Başlatılmayı bekleyen (`queued`) çağrılar.
    async fn pending_call_starts(&self, limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        let _ = limit;
        Ok(Vec::new())
    }

    /// Çağrılan bitmiş, çağıranın işlemesi bekleniyor (`returned`). Yalnız `wait`.
    async fn pending_call_returns(&self, limit: i64) -> Result<Vec<PendingCall>, EngineError> {
        let _ = limit;
        Ok(Vec::new())
    }

    /// Süre sınırı geçmiş `running` `wait` çağrıları — `$call.status = "timeout"`.
    async fn overdue_calls(
        &self,
        now: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<PendingCall>, EngineError> {
        let (_, _) = (now, limit);
        Ok(Vec::new())
    }

    /// Çağrı satırının durumunu değiştirir. `callee` verilirse satıra yazılır.
    /// Terminal durumlar: `consumed` (dönüş işlendi), `failed`, `cancelled`, `skipped`.
    async fn set_call_status(
        &self,
        call_row_id: Uuid,
        status: &str,
        callee_wfe_id: Option<Uuid>,
    ) -> Result<(), EngineError> {
        let (_, _, _) = (call_row_id, status, callee_wfe_id);
        Ok(())
    }

    /// WFC-CASCADE: çağıran sonlandığında koşan ALT AKIŞLARI iptal eder.
    /// **Ardılı KAPSAMAZ** — ardıl, astın aksine çağıranın ömrüne bağlı değildir.
    /// Döndürülen id'ler iptal edilecek çağrılan WFE'lerdir (executor sonlandırır).
    async fn cancel_subcalls_of(&self, caller_wfe_id: Uuid) -> Result<Vec<Uuid>, EngineError> {
        let _ = caller_wfe_id;
        Ok(Vec::new())
    }

    /// Bir WFE'nin YAPTIĞI çağrılar — API görünümü (`GET /wfe/:id` → `calls`).
    async fn calls_of_caller(&self, caller_wfe_id: Uuid) -> Result<Vec<CallView>, EngineError> {
        let _ = caller_wfe_id;
        Ok(Vec::new())
    }

    /// Bu WFE'yi ÇAĞIRAN kayıt (varsa) — `GET /wfe/:id` → `caller`.
    /// Portal "bu iş şu akıştan geldi" / "ardıl akış" kartlarını buradan kurar.
    async fn caller_of(&self, callee_wfe_id: Uuid) -> Result<Option<CallView>, EngineError> {
        let _ = callee_wfe_id;
        Ok(None)
    }

    /// Bir WFE terminal'e ulaştığında, onu bekleyen çağrı satırını `returned`'e çeker
    /// (`wait`) ya da doğrudan `consumed` yapar (`detached`/`terminal`). Bekleyen yoksa
    /// hiçbir şey yapmaz. `status`: "completed" | "failed" | "terminated".
    async fn mark_callee_finished(
        &self,
        callee_wfe_id: Uuid,
        status: &str,
        end_response: Option<&Value>,
    ) -> Result<(), EngineError> {
        let (_, _, _) = (callee_wfe_id, status, end_response);
        Ok(())
    }
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
