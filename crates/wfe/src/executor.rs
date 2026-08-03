//! v2.2 WfeExecutor — saf Engine (wfe-core::v22::pipeline) ile store'lar arasındaki
//! ince orkestrasyon katmanı. Tüm yazımlar WfeStore'un atomik create/commit/claim'i
//! üzerinden gider (M8).

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;
use wfe_core::types::actor::{Actor, CandidateActor};
use wfe_core::types::wfah::WfahEntry;
use wfe_core::types::wfd_v22::{CallMode, JoinRule, StartAs, Wfd, WftTarget};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::matcher::{AuthDecision, MatchEnv};
use wfe_core::v22::pipeline::{ClaimCheck, ClaimTimeoutOutcome, Engine};
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CallView, CommitOutcome, PendingCall, WfdStore,
    WfeStore, Wfes,
};
use wfe_core::v22::visibility::{can_view, filter_dynctx};
use wfe_core::{ConflictKind, EngineError, OrgPort};

/// SLA-1 (2026-07-16): `claimed_at + node.claim_timeout.after`; claim yoksa,
/// current_node yoksa veya node claim_timeout taşımıyorsa `None`. `WfeView` ve
/// pool/liste görünümlerinin ortak hesabı.
pub fn compute_claim_deadline(
    wfd: &Wfd,
    current_node: Option<&str>,
    claimed_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let claimed_at = claimed_at?;
    let node = wfd.nodes.get(current_node?)?;
    let ct = node.claim_timeout.as_ref()?;
    let after = wfe_core::v22::duration::parse_iso8601_duration(&ct.after).ok()?;
    Some(claimed_at + after)
}

/// WOR-65: istemcinin taşıdığı revizyon token'ı (`expected_rev`) yüklenen durumun
/// revizyonuyla (`Wfes::rev()` — son WFAH seq'i) uyuşuyor mu?
///
/// `None` = istemci token göndermedi → kontrol YOK, bugünkü davranış aynen sürer
/// (geriye dönük uyumluluk sözleşmesi). `Some(n)` ve uyuşmazlık → kalıcı bir
/// precondition ihlali: durum istemcinin okuduğu andan beri değişmiştir
/// (collapse, kardeş kol, escalation, timer, başka bir kullanıcının aksiyonu…).
/// Retry ANLAMSIZDIR — reload aynı uyuşmazlığı üretir — bu yüzden çağıranlar
/// `Conflict(StaleRevision)`'ı retry döngüsüne SOKMADAN yukarı verir.
fn check_rev(wfes: &Wfes, expected_rev: Option<u32>) -> Result<(), EngineError> {
    match expected_rev {
        Some(expected) if expected != wfes.rev() => {
            Err(EngineError::Conflict(ConflictKind::StaleRevision))
        }
        _ => Ok(()),
    }
}

/// İdempotent re-claim kontrolü: verilen aktör zaten sahip mi? Paralel modda
/// (`node` verilir) o kolun `claimed_by`'ı, aksi halde wfe-seviyesi `assigned_to`.
fn branch_owner_is(wfes: &Wfes, node: Option<&str>, user_id: Uuid) -> bool {
    match node {
        Some(n) => {
            wfes.branches
                .iter()
                .find(|b| b.status == BranchStatus::Active && b.branch_node == n)
                .and_then(|b| b.claimed_by)
                == Some(user_id)
        }
        None => wfes.assigned_to == Some(user_id),
    }
}

/// WOR-31: WFE'nin şu an aktif kollarının node adları (paralel timer taraması
/// bunlar üzerinden döner). Paralel modda değilse boş. `pub`: sim rotaları da
/// kol-bazlı uygunluk/aksiyon listesi için kullanır.
pub fn active_branch_nodes(wfes: &Wfes) -> Vec<String> {
    wfes.branches
        .iter()
        .filter(|b| b.status == BranchStatus::Active)
        .map(|b| b.branch_node.clone())
        .collect()
}

/// T4 (API/sim): mümkün aksiyon — paralel modda hangi kola ait olduğunu belirten
/// opsiyonel `node` etiketiyle. Paralel-olmayan modda `node` her zaman `None`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PossibleAction {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
}

/// T4: `Engine::possible_actions`'ın paralel-farkında sarmalayıcısı — paralel
/// modda TÜM aktif kollar için ayrı çağrı yapılıp birleşim (her öğe kendi kol
/// node'uyla etiketli) döner; paralel değilse tek çağrı, `node: None`. Hem
/// `WfeExecutor::possible_actions` hem de `routes/simulate.rs` (store'suz sim)
/// bu ortak yardımcıyı kullanır.
pub async fn possible_actions_for(
    engine: &Engine<'_>,
    wfd: &Wfd,
    wfes: &Wfes,
    actor: &Actor,
) -> Result<Vec<PossibleAction>, EngineError> {
    if wfes.join_target.is_some() {
        let mut out = Vec::new();
        for node in active_branch_nodes(wfes) {
            let actions = engine
                .possible_actions(wfd, wfes, actor, Some(&node))
                .await?;
            out.extend(actions.into_iter().map(|action| PossibleAction {
                action,
                node: Some(node.clone()),
            }));
        }
        Ok(out)
    } else {
        let actions = engine.possible_actions(wfd, wfes, actor, None).await?;
        Ok(actions
            .into_iter()
            .map(|action| PossibleAction { action, node: None })
            .collect())
    }
}

/// Commit outcome'unun API görünümü: (terminal, current_node, end_response).
/// WOR-31 paralel outcome'ları: fork/kol hareketi/kol varışı aktif WFE'dir ve
/// wfe-seviyesi current_node taşımaz (kol durumu T3/T4'te ayrıca sunulur);
/// `JoinComplete` iç `next` outcome'una göre sınıflanır.
fn outcome_view(outcome: &CommitOutcome) -> (bool, Option<String>, Option<Value>) {
    match outcome {
        CommitOutcome::MoveTo { node } => (false, Some(node.clone()), None),
        CommitOutcome::Terminal { end_response } => (true, None, Some(end_response.clone())),
        CommitOutcome::Failed { end_response } => (true, None, Some(end_response.clone())),
        CommitOutcome::Terminated { end_response } => (true, None, Some(end_response.clone())),
        CommitOutcome::ForkTo { .. }
        | CommitOutcome::BranchMoveTo { .. }
        | CommitOutcome::BranchArrived { .. } => (false, None, None),
        CommitOutcome::JoinComplete { next, .. } => outcome_view(next),
        // WOR-56: node hedefli collapse — paralel mod biter, WFE aktif olarak `node`'a.
        CommitOutcome::CollapseTo { node, .. } => (false, Some(node.clone()), None),
    }
}

pub struct WfeExecutor {
    pub org: Arc<dyn OrgPort>,
    pub wfd: Arc<dyn WfdStore>,
    pub wfe: Arc<dyn WfeStore>,
    pub runner: Arc<dyn AutoexecRunner>,
    /// Event-driven SLA timer sinyali (bkz. `crate::timer`): create/commit/claim
    /// sonrası dürtülür; timer servisi next-due'yu yeniden hesaplar. `notify_one`
    /// permit biriktirdiği için sweep sırasında gelen sinyal KAYBOLMAZ.
    timer_notify: Arc<tokio::sync::Notify>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeStartResult {
    pub wfe_id: Uuid,
    pub terminal: bool,
    pub current_node: Option<String>,
    pub end_response: Option<Value>,
    pub current_c_a: Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeApplyResult {
    pub wfe_id: Uuid,
    pub terminal: bool,
    pub current_node: Option<String>,
    pub end_response: Option<Value>,
    pub current_c_a: Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeView {
    pub wfe_id: Uuid,
    pub status: WfeStatus,
    pub current_node: Option<String>,
    pub claimed_by: Option<Uuid>,
    pub dynctx: Value,
    pub wfah: Vec<wfe_core::types::wfah::WfahEntry>,
    pub end_response: Option<Value>,
    /// SLA-3: çözülmüş mutlak workflow deadline'ı; NULL = yok.
    pub deadline: Option<DateTime<Utc>>,
    /// SLA-1: en son claim anı.
    pub claimed_at: Option<DateTime<Utc>>,
    /// SLA-1: `claimed_at + node.claim_timeout.after` (hesaplanmış); n/a ise NULL.
    pub claim_deadline: Option<DateTime<Utc>>,
    /// 1–10, deadline'dan otomatik hesaplanır (bkz. `priority::compute_priority`).
    pub priority: i32,
    /// WOR-31 T4: paralel mod kol durumları — paralel modda değilken boş.
    /// JSON alan adı `node` (bkz. `BranchState`), artı sorgu-anında çözülmüş `c_a`.
    pub branches: Vec<BranchView>,
    /// WOR-31 T4: fork'ta persist edilen join hedefi; `Some` = paralel mod
    /// (bu durumda `current_node` `None`'dur).
    pub join_target: Option<WftTarget>,
    /// WOR-72/WOR-73: join kuralının kısa adı — `"and"` | `"or"` | `"expr"`.
    /// İstemci hangi mantığın işlediğini buradan bilir (paralel modda değilken
    /// `join_target` gibi anlamsızdır, o yüzden yalnız paralel modda gönderilir).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_mode: Option<&'static str>,
    /// WOR-72: quorum eşiği (yalnız `join_mode: "or"`) — istemci "3 kolun 2'si yeter"
    /// bilgisini buradan gösterir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_threshold: Option<u32>,
    /// WOR-73: ZEN join koşulu (yalnız `join_mode: "expr"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_when: Option<String>,
    /// Madde 6: tek-kol modda viewer `current_node`'u claim edebilir mi ve NASIL
    /// (paralel modda daima `None` — kol-bazlı `BranchView.claim_as`'e bak). `None`
    /// = claim edemez / zaten claim'li / claim aşaması değil.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_as: Option<ClaimProvenance>,
    /// WOR-65: WFE revizyon token'ı (bkz. `Wfes::rev()`). İstemci bunu okuyup
    /// bir sonraki apply/claim'de `expected_rev` olarak geri gönderirse, arada
    /// durum değişmişse 409 `conflict.stale_revision` alır. WFE-seviyesidir —
    /// paralel modda TÜM kollar için aynı değerdir.
    pub rev: u32,
    /// WFC: bu WFE'nin YAPTIĞI iş akışı çağrıları (alt akışlar + ardıl).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<CallView>,
    /// WFC: bu WFE'yi başlatan çağrı — "bu iş şu akıştan geldi". Kök WFE'de `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<CallView>,
}

/// GET /wfe/:id kol görünümü: kalıcı `BranchState` alanları (`#[serde(flatten)]` ile
/// `node`/`status`/`claimed_by`/`claimed_at`/`entered_at`) + sorgu-anında çözülmüş
/// `c_a` (bu kolu kim claim edebilir — tek-kol `current_c_a`'nın kol karşılığı).
/// `c_a` PERSIST EDİLMEZ; yalnız aktif kollar için doldurulur (arrived/cancelled boş).
#[derive(Debug, serde::Serialize)]
pub struct BranchView {
    #[serde(flatten)]
    pub state: BranchState,
    pub c_a: Vec<CandidateActor>,
    /// Madde 6: viewer bu kolu claim edebilir mi ve NASIL — sorgu-anında
    /// `claim_decision` ile. `None` = claim edemez / kol claim'li / claim aşaması
    /// değil. `delegated` ise UI "X adına vekaleten" gösterir.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_as: Option<ClaimProvenance>,
}

/// Viewer'ın bir (kol) node'unu claim edebilirliği (Madde 6 vekalet-farkında).
/// `AuthDecision`'ın serileşen görünüm karşılığı; `Denied` → `None` (alan düşer).
#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ClaimProvenance {
    /// Aktör kurala DOĞRUDAN uyuyor.
    Direct,
    /// Aktör VEKALETEN uyuyor: `delegator_user_id`'nin koltuğunu (`seat_*`) temsil eder.
    Delegated {
        delegation_id: Uuid,
        delegator_user_id: Uuid,
        seat_orgu_id: Uuid,
        seat_role: String,
    },
}

impl ClaimProvenance {
    fn from_auth(d: AuthDecision) -> Option<Self> {
        match d {
            AuthDecision::Direct => Some(ClaimProvenance::Direct),
            AuthDecision::Delegated {
                delegation_id,
                delegator_user_id,
                seat_orgu_id,
                seat_role,
            } => Some(ClaimProvenance::Delegated {
                delegation_id,
                delegator_user_id,
                seat_orgu_id,
                seat_role,
            }),
            AuthDecision::Denied => None,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct ClaimOutcome {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Madde 7: devir sonucu. Reddetmeler (yetkisiz / hedef uygun değil / terminal)
/// `EngineError` olarak döner (HTTP 4xx); başarı her zaman `success: true`.
#[derive(Debug, serde::Serialize)]
pub struct ReassignOutcome {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl WfeExecutor {
    pub fn new(
        org: Arc<dyn OrgPort>,
        wfd: Arc<dyn WfdStore>,
        wfe: Arc<dyn WfeStore>,
        runner: Arc<dyn AutoexecRunner>,
    ) -> Self {
        Self {
            org,
            wfd,
            wfe,
            runner,
            timer_notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn engine(&self) -> Engine<'_> {
        Engine {
            org: &*self.org,
            exec: &*self.runner,
        }
    }

    /// Timer servisinin dinlediği sinyal kanalı (bkz. `crate::timer::run_timer_service`).
    pub fn timer_signal(&self) -> Arc<tokio::sync::Notify> {
        self.timer_notify.clone()
    }

    /// SLA zamanlayıcılarını etkileyen her kalıcı mutasyondan sonra çağrılır —
    /// timer servisi uyanıp en yakın vadeyi yeniden hesaplar.
    fn nudge_timers(&self) {
        self.timer_notify.notify_one();
    }

    /// `deadline`: SLA-3 — başlatan kullanıcının opsiyonel ISO 8601 duration'ı
    /// (bkz. `Engine::start`; `wfd.timeout` tavanına tabidir).
    pub async fn start(
        &self,
        wfd_id: Uuid,
        version: i32,
        actor: &Actor,
        action: Option<&str>,
        input: &Value,
        deadline: Option<&str>,
    ) -> Result<WfeStartResult, EngineError> {
        let wfd = self.wfd.fetch(wfd_id, version).await?;
        let orgtnt_id = self.org.orgtnt_for_orgu(actor.orgu_id).await?;

        // wfe_id ÖNCE üretilir; $wfe_id effects gerçek id ile çözülür (WOR-6)
        let wfe_id = Uuid::new_v4();
        let mut new = self
            .engine()
            .start(&wfd, actor, orgtnt_id, action, input, wfe_id, deadline)
            .await?;
        new.wfd_id = wfd_id;
        new.wfd_version = version;

        self.wfe.create(&new).await?;
        self.nudge_timers(); // deadline / node dwell / claim_timeout vadesi değişti

        let (terminal, current_node, end_response) = outcome_view(&new.outcome);
        Ok(WfeStartResult {
            wfe_id,
            terminal,
            current_node,
            end_response,
            current_c_a: new.resolved_c_a,
        })
    }

    /// `node_hint`: WOR-31 — paralel modda kol seçimi (API body `node`; T4 bağlar).
    ///
    /// `expected_rev`: WOR-65 — istemcinin okuduğu WFE revizyon token'ı (API body
    /// `expected_rev`). `None` = kontrol yok (bugünkü davranış). `Some(n)` ve durum
    /// bu arada ilerlediyse hiçbir şey uygulanmaz, `Conflict(StaleRevision)` döner.
    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
        action: &str,
        input: &Value,
        node_hint: Option<&str>,
        expected_rev: Option<u32>,
    ) -> Result<WfeApplyResult, EngineError> {
        // WOR-31: paralel modda eşzamanlı kol hareketleri (BranchMoveTo/
        // BranchArrived/JoinComplete/CollapseTo) adapter FOR UPDATE + kol CAS +
        // aktif-kol sayımı ile serialize edilir; uyumsuzlukta `Conflict(kind)`
        // döner. Engine SAFtır (yalnız kendi görüşünü emit eder), yarış burada
        // reload + yeniden-koşma ile çözülür. En çok 3 deneme: her tur bir kolu
        // kesin ilerletir, sonlu kol sayısı sonluluğu garanti eder.
        //
        // WOR-62: her Conflict retry EDİLMEZ. `ConflictKind::is_retryable()`
        // false ise (örn. `Collapsed`: paralel mod kalıcı olarak bitti) reload
        // aynı verdikti üretir — boşuna 3 tur dönüp KEYFİ bir engine hatası
        // (TransitionNotFound / AmbiguousAction / PermissionDenied, hangi node'a
        // collapse edildiğine bağlı) döndürmek yerine conflict'i AYNEN yukarı
        // verir. Kaybeden kardeş aksiyon böylece deterministik olarak
        // 409 + `code: "conflict.collapsed"` alır.
        const MAX_ATTEMPTS: u32 = 3;
        let mut last_err = None;
        for _ in 0..MAX_ATTEMPTS {
            let wfes = self.wfe.load(wfe_id).await?;
            // WOR-65: revizyon kapısı engine'i koşturmadan ÖNCE — istemcinin
            // gördüğü durum artık geçerli değilse hiçbir yan etki üretilmemeli.
            // `?` ile ERKEN döner: retry döngüsüne GİRMEZ (bkz. `check_rev`).
            // Retry turlarında da geçerlidir ve orada asıl işini yapar: örtük bir
            // seq çakışmasından sonra reload taze duruma bakar, istemcinin
            // precondition'ı artık tutmadığı için aksiyon SESSİZCE uygulanmaz.
            check_rev(&wfes, expected_rev)?;
            let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

            let commit = self
                .engine()
                .apply(&wfd, &wfes, actor, action, input, node_hint)
                .await?;
            match self.wfe.commit(&commit).await {
                Ok(()) => {
                    // WFC: bu WFE sonlandıysa onu bekleyen çağrıyı `returned`'e çek ve
                    // kendi alt akışlarını iptal et. **nudge'dan ÖNCE** olmak zorunda:
                    // ters sırada sweeper uyanıp satırı henüz `running` görüyor, hiçbir
                    // şey bulmadan tekrar uykuya dalıyor ve çağıranın dönüşü 60 sn'lik
                    // güvenlik ağına kadar gecikiyordu.
                    self.after_wfe_settled(wfe_id, &commit.outcome).await?;
                    // node değişimi escalation dwell'ini ve claim sayacını sıfırlar;
                    // ayrıca yukarıda yazılan çağrı dönüşünü sweeper'a duyurur.
                    self.nudge_timers();
                    let (terminal, current_node, end_response) = outcome_view(&commit.outcome);
                    return Ok(WfeApplyResult {
                        wfe_id,
                        terminal,
                        current_node,
                        end_response,
                        current_c_a: commit.resolved_c_a,
                    });
                }
                Err(EngineError::Conflict(kind)) if kind.is_retryable() => {
                    last_err = Some(EngineError::Conflict(kind));
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        Err(last_err.unwrap_or(EngineError::Conflict(ConflictKind::BranchArrival)))
    }

    /// Claim uygunluğu — matcher tabanlı (§7.1); c_u kuralları dahil doğru çalışır.
    /// `node`: WOR-31 — paralel modda kol node'u; uygunluk O KOLUN c_a'sına ve
    /// kol claim durumuna göre değerlendirilir. `None` paralel-olmayan davranış.
    pub async fn can_claim(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
        node: Option<&str>,
    ) -> Result<(bool, Option<String>), EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        // İdempotent re-claim: zaten sahipse (kol veya wfe-seviyesi) başarı.
        if branch_owner_is(&wfes, node, actor.user_id) {
            return Ok((true, None));
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        Ok(
            match self.engine().can_claim(&wfd, &wfes, actor, node).await? {
                ClaimCheck::Ok => (true, None),
                ClaimCheck::AlreadyClaimed => (false, Some("already_claimed".into())),
                ClaimCheck::Terminal => (false, Some("terminal".into())),
                ClaimCheck::Expired => (false, Some("expired".into())),
                ClaimCheck::NotEligible => (false, Some("not_eligible".into())),
            },
        )
    }

    /// Atomik claim: uygunluk matcher ile doğrulanır, yazım CAS ile yapılır.
    /// `node`: WOR-31 — paralel modda kol node'u (CAS o kolda yapılır).
    ///
    /// `expected_rev`: WOR-65 — opsiyonel revizyon token'ı. Claim'in KENDİ CAS'ı
    /// (`claimed_by IS NULL`) eşzamanlı claim yarışını zaten çözer; revizyon
    /// kapısının çözdüğü AYRI bir sorundur: kullanıcının listede gördüğü satır
    /// bu arada geçersizleşmiş olabilir (collapse kolu `cancelled` yaptı, WFE
    /// başka node'a geçti). Kapı olmadan bu durum ayırt edilemeyen bir
    /// `already_claimed` / `not_eligible` gerekçesine düşer; kapıyla NET bir
    /// `conflict.stale_revision` olur. `None` = kontrol yok, ek sorgu da yok.
    pub async fn claim(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
        node: Option<&str>,
        expected_rev: Option<u32>,
    ) -> Result<ClaimOutcome, EngineError> {
        // Revizyon kapısı uygunluk kontrolünden ÖNCE: stale bir istek için
        // "başkası aldı" demek yanıltıcıdır, doğru cevap "durum değişti"dir.
        if expected_rev.is_some() {
            check_rev(&self.wfe.load(wfe_id).await?, expected_rev)?;
        }
        let (eligible, reason) = self.can_claim(wfe_id, actor, node).await?;
        if !eligible {
            return Ok(ClaimOutcome {
                success: false,
                reason,
            });
        }
        let wfes = self.wfe.load(wfe_id).await?;
        if branch_owner_is(&wfes, node, actor.user_id) {
            return Ok(ClaimOutcome {
                success: true,
                reason: None,
            });
        }
        // Madde 6: claim DOĞRUDAN mı VEKALETEN mi uygun? Vekaletense CAS kazanılınca
        // aynı transaction'da `claim:delegated` audit marker'ı yazılır.
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let marker = match self
            .engine()
            .claim_decision(&wfd, &wfes, actor, node)
            .await?
        {
            AuthDecision::Delegated {
                delegation_id,
                delegator_user_id,
                seat_orgu_id,
                seat_role,
            } => {
                let seq = wfes.wfah.entries().last().map(|e| e.seq + 1).unwrap_or(1);
                Some(WfahEntry {
                    seq,
                    action: "claim:delegated".into(),
                    actor: actor.clone(),
                    input: Some(serde_json::json!({
                        "delegation_id": delegation_id.to_string(),
                        "delegator": delegator_user_id.to_string(),
                        "seat": { "orgu_id": seat_orgu_id.to_string(), "role": seat_role },
                    })),
                    applied_at: Utc::now(),
                })
            }
            _ => None,
        };
        let won = self
            .wfe
            .claim(wfe_id, wfes.orgtnt_id, actor.user_id, node, marker.as_ref())
            .await?;
        if won {
            self.nudge_timers(); // claim_timeout sayacı şimdi başladı (SLA-1)
        }
        Ok(ClaimOutcome {
            success: won,
            reason: if won {
                None
            } else {
                Some("already_claimed".into())
            },
        })
    }

    /// Madde 7: yetkili claim devri. `reassigner` = X-Actor-* (amir), `target` =
    /// devralacak tam aktör üçlüsü (`None` = havuza bırakma). Uygunluk
    /// `Engine::reassign`'da doğrulanır (reassign c_a + hedef node c_a); denetim
    /// için WFAH marker yazılır. `node`: WOR-31 paralel modda kol seçimi.
    pub async fn reassign(
        &self,
        wfe_id: Uuid,
        reassigner: &Actor,
        target: Option<&Actor>,
        node: Option<&str>,
    ) -> Result<ReassignOutcome, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let entry = self
            .engine()
            .reassign(&wfd, &wfes, reassigner, target, node, Utc::now())
            .await?;
        self.wfe
            .reassign(
                wfe_id,
                wfes.orgtnt_id,
                target.map(|a| a.user_id),
                &entry,
                node,
            )
            .await?;
        self.nudge_timers(); // hedefli devirde claim_timeout sayacı (SLA-1) yeniden başladı
        Ok(ReassignOutcome {
            success: true,
            reason: None,
        })
    }

    /// WFE görünümü — önce WFE-seviyesi VIEW kapısı (owner / node c_a / listable,
    /// spec Terminology VISIBILITY+LISTABLE), sonra DynCtx `x-visibility` field
    /// filtrelemesi (M13). Kapı geçilmezse WFE'nin varlığı bile sızmaz.
    pub async fn query(&self, wfe_id: Uuid, viewer: &Actor) -> Result<WfeView, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        if !can_view(&wfd, &wfes, viewer, &*self.org).await? {
            return Err(EngineError::Unauthorized);
        }

        let ctx = wfes.dynctx.as_value();
        let env = MatchEnv {
            ctx,
            wfah: &wfes.wfah,
            orgtnt_id: wfes.orgtnt_id,
        };
        let filtered = filter_dynctx(&wfd.context, ctx, viewer, env, &*self.org).await?;

        let now = Utc::now();
        let priority = crate::priority::compute_priority(wfes.created_at, wfes.deadline, now);
        let claim_deadline =
            compute_claim_deadline(&wfd, wfes.current_node.as_deref(), wfes.claimed_at);
        // `wfes` aşağıda parça parça taşınıyor — revizyon ÖNCE okunmalı (WOR-65).
        let rev = wfes.rev();

        // Kol görünümü: her AKTİF kol için c_a'yı sorgu-anında çöz (tek-kol
        // `current_c_a`'nın kol karşılığı; parent/*:[type:]/anchor formları burada
        // motorun org resolver'ıyla doğru çözülür — portal bunu düz okur). Anchor'sız
        // Selector `self` için default anchor = viewer; anchor-tabanlı formlarda viewer
        // önemsiz. arrived/cancelled kollarda c_a boş (claim edilemezler).
        // Madde 6: aktif & claim'siz kol için viewer'ın claim provenance'ı (direct/
        // delegated) — `claim_decision` `&wfes`'in tamamını istediği için kollar
        // REFERANSLA dönülür (branch state klonlanır); wfes bozulmaz.
        let engine = self.engine();
        let mut branch_views = Vec::with_capacity(wfes.branches.len());
        for b in &wfes.branches {
            let active = b.status == BranchStatus::Active;
            let c_a = if active {
                engine
                    .resolve_node_c_a(
                        &wfd,
                        &b.branch_node,
                        ctx,
                        &wfes.wfah,
                        viewer,
                        wfes.orgtnt_id,
                    )
                    .await?
            } else {
                Vec::new()
            };
            let claim_as = if active && b.claimed_by.is_none() {
                ClaimProvenance::from_auth(
                    engine
                        .claim_decision(&wfd, &wfes, viewer, Some(&b.branch_node))
                        .await?,
                )
            } else {
                None
            };
            branch_views.push(BranchView {
                state: b.clone(),
                c_a,
                claim_as,
            });
        }

        // Madde 6: tek-kol modda (join_target yok) viewer current_node'u claim
        // edebilir mi — aktif & henüz claim'siz ise. Paralel modda daima None.
        let claim_as = if wfes.join_target.is_none()
            && wfes.status == WfeStatus::Active
            && wfes.assigned_to.is_none()
            && wfes.current_node.is_some()
        {
            ClaimProvenance::from_auth(engine.claim_decision(&wfd, &wfes, viewer, None).await?)
        } else {
            None
        };

        // WFC: bu WFE'nin çağrıları + onu başlatan çağrı. Store bunları desteklemiyorsa
        // (WFC'siz kurulum / test store) boş küme döner — görünüm alanları hiç yazılmaz.
        let calls = self.wfe.calls_of_caller(wfe_id).await?;
        let caller = self.wfe.caller_of(wfe_id).await?;

        Ok(WfeView {
            wfe_id,
            status: wfes.status,
            current_node: wfes.current_node,
            claimed_by: wfes.assigned_to,
            dynctx: filtered,
            wfah: wfes.wfah.entries().to_vec(),
            end_response: wfes.end_response,
            deadline: wfes.deadline,
            claimed_at: wfes.claimed_at,
            claim_deadline,
            priority,
            rev,
            branches: branch_views,
            join_mode: wfes
                .join_target
                .as_ref()
                .map(|_| wfes.join_rule.kind()),
            join_threshold: match &wfes.join_rule {
                JoinRule::Quorum(k) => Some(*k),
                _ => None,
            },
            join_when: match &wfes.join_rule {
                JoinRule::Expr(e) => Some(e.clone()),
                _ => None,
            },
            join_target: wfes.join_target,
            claim_as,
            calls,
            caller,
        })
    }

    /// WOR-31 T4: paralel modda TÜM aktif kollar için birleşim döner, her öğe
    /// kendi kol `node`'uyla etiketli (bkz. `possible_actions_for`).
    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
    ) -> Result<Vec<PossibleAction>, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status == WfeStatus::Terminal {
            return Ok(vec![]);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        possible_actions_for(&self.engine(), &wfd, &wfes, actor).await
    }

    /// Tek WFE için zamanlayıcı kontrolü — sıra (2026-07-16 SLA sözleşmesi):
    /// 1. instance deadline (SLA-3 — akışı `terminated` yapan TEK kural),
    /// 2. claim timeout (SLA-1), 3. escalation (SLA-2 — yalnız node devri).
    /// Bir şey ateşlendiyse true döner
    /// (M5/M6 — WOR-46/47).

    // ================================================================ WFC
    //
    // İş akışı çağrısı (WFC) — üç mod: `wait` / `detached` (alt akış, node yerleşimi)
    // ve `terminal` (ardıl akış). Plan: docs/plans/workflow-call.md.
    //
    // Çalışma modeli OUTBOX'tır: çağrı niyeti çağıranın commit'i ile AYNI tx'te
    // `wf.wfe_call`'a `queued` olarak yazılır (bkz. `WfeAdapter::commit`), gerçek start
    // burada AYRI bir tx'te koşar. Böylece çağıranın atomikliği başka bir WFE'nin tüm
    // start pipeline'ına bağlanmaz ve başlatma yeniden denenebilir olur.

    /// Alt akış yuvalanma sınırı. `call_cycle` statik olarak kaçarsa (pin'siz sürüm →
    /// "en son"a göre en iyi çaba) runtime freni budur.
    const MAX_CALL_DEPTH: i32 = 8;
    /// Ardıl zinciri sınırı. Ardıl döngüsü (A bitince B, B bitince A) sonsuz WFE
    /// üretir — sitedeki `max_next` ile birlikte iki katmanlı frenin ikinci katmanı.
    const MAX_NEXT_DEPTH: i32 = 16;

    /// Kuyruktaki çağrıları başlatır. Dönen sayı başlatılan çağrı adedidir.
    ///
    /// Bir çağrının başlatılamaması çağıranı ETKİLEMEZ (Handoff Isolation): satır
    /// `failed`/`skipped` olur, çağıranın durumu neyse öyle kalır.
    pub async fn run_pending_calls(&self, limit: i64) -> Result<usize, EngineError> {
        let pending = self.wfe.pending_call_starts(limit).await?;
        let mut started = 0usize;
        for call in pending {
            match self.start_one_call(&call).await {
                Ok(true) => started += 1,
                Ok(false) => {}
                Err(e) => {
                    // Çağıranı bozmadan kaydet ve devam et — bir çağrının patlaması
                    // diğerlerini engellemesin.
                    tracing::warn!(
                        "WFC start başarısız (call_row={}, caller={}, key={}): {e}",
                        call.id,
                        call.caller_wfe_id,
                        call.call_key
                    );
                    let _ = self.wfe.set_call_status(call.id, "failed", None).await;
                }
            }
        }
        Ok(started)
    }

    /// Tek bir çağrıyı başlatır. `Ok(false)` = derinlik sınırı ya da eskimiş satır
    /// yüzünden bilinçli olarak başlatılmadı.
    async fn start_one_call(&self, call: &PendingCall) -> Result<bool, EngineError> {
        // Derinlik frenleri. Aşılırsa çağrılan HİÇ başlatılmaz; çağıran `completed`
        // (ya da bulunduğu durumda) kalır ve satır `skipped` olur.
        let is_next = matches!(call.mode, CallMode::Terminal);
        let over_depth = if is_next {
            let cap = call
                .max_next
                .map(|m| m as i32)
                .unwrap_or(Self::MAX_NEXT_DEPTH)
                .min(Self::MAX_NEXT_DEPTH);
            call.next_depth > cap
        } else {
            call.depth > Self::MAX_CALL_DEPTH
        };
        if over_depth {
            tracing::warn!(
                "WFC derinlik sınırı aşıldı (call_row={}, mode={}, depth={}, next_depth={}) — başlatılmadı",
                call.id,
                call.mode.as_str(),
                call.depth,
                call.next_depth
            );
            self.wfe.set_call_status(call.id, "skipped", None).await?;
            return Ok(false);
        }

        let caller = self.wfe.load(call.caller_wfe_id).await?;
        let caller_wfd = self.wfd.fetch(caller.wfd_id, caller.wfd_version).await?;
        let def = caller_wfd.calls.get(&call.call_key).ok_or_else(|| {
            EngineError::CallNotFound(format!(
                "'{}' çağıranın calls katalogunda yok",
                call.call_key
            ))
        })?;

        // Doküman kimliği → yayınlanmış (uuid, version).
        let (callee_wfd_id, callee_version) = self
            .wfd
            .resolve_doc(caller.orgtnt_id, &def.wfd_id, def.version.as_deref())
            .await?
            .ok_or_else(|| {
                EngineError::CallNotFound(format!(
                    "çağrılan akış bulunamadı: '{}'{}",
                    def.wfd_id,
                    def.version
                        .as_deref()
                        .map(|v| format!(" @{v}"))
                        .unwrap_or_default()
                ))
            })?;

        // Çağrılanı hangi aktör başlatır (plan §9.1):
        //   actor  → çağıranı bu noktaya getiren ACT'in aktörü (WFAH'ın SON kaydı)
        //   system → akışı BAŞLATAN aktör (WFAH'ın İLK kaydı)
        //
        // "system" için nil bir sistem aktörü kullanılmaz: hiçbir `c_a` ile eşleşmez,
        // yani ardıl asla başlayamazdı. Akışın başlatıcısı hem gerçek bir kullanıcıdır
        // hem denetim izini anlamlı tutar. Eşleşmezse `WFD.CallUnauthorized` — çağıranın
        // sonucu DEĞİŞMEZ, hata yalnız çağrı satırında görünür.
        let entries = caller.wfah.entries();
        let actor = match (call.start_as, entries.first(), entries.last()) {
            (StartAs::System, Some(first), _) => first.actor.clone(),
            (_, _, Some(last)) => last.actor.clone(),
            (_, Some(first), None) => first.actor.clone(),
            _ => {
                return Err(EngineError::CallUnauthorized(
                    "çağıranın WFAH'ı boş — başlatacak aktör yok".into(),
                ))
            }
        };

        // `queued` → `running` ÖNCE yazılır: start başarısız olsa bile satır bir daha
        // kuyruktan okunmaz (yeniden denenmesi gerekiyorsa hata yolu `failed` yazar).
        self.wfe.set_call_status(call.id, "running", None).await?;

        // `CallDef.start` bir startRule **id**'sidir; `Engine::start` ise kuralı ACTION
        // adıyla seçer (birden fazla start kuralı olabilir). Bu yüzden çağrılanın WFD'si
        // okunup id → action eşlemesi yapılır. Verilmemişse `None` = ilk uygun kural.
        let start_action = match &def.start {
            Some(rule_id) => {
                let callee_wfd = self.wfd.fetch(callee_wfd_id, callee_version).await?;
                match callee_wfd.start.iter().find(|r| r.id == *rule_id) {
                    Some(rule) => Some(rule.action.clone()),
                    None => {
                        self.wfe.set_call_status(call.id, "failed", None).await?;
                        return Err(EngineError::CallNotFound(format!(
                            "çağrılan '{}' akışında '{rule_id}' başlatma kuralı yok",
                            def.wfd_id
                        )));
                    }
                }
            }
            None => None,
        };

        let started = self
            .start(
                callee_wfd_id,
                callee_version,
                &actor,
                start_action.as_deref(),
                &call.input,
                None,
            )
            .await;

        match started {
            Ok(res) => {
                self.wfe
                    .set_call_status(call.id, "running", Some(res.wfe_id))
                    .await?;
                // Çağrılan ANINDA bitmiş olabilir (tam otomatik akış) — dönüşü
                // beklemeye bırakmayıp hemen işaretle. `wait` kadar hızlı olmasının
                // sırrı bu: `sync` moduna gerek kalmaz.
                if res.terminal {
                    self.wfe
                        .mark_callee_finished(res.wfe_id, "completed", res.end_response.as_ref())
                        .await?;
                }
                Ok(true)
            }
            Err(e) => {
                self.wfe.set_call_status(call.id, "failed", None).await?;
                Err(e)
            }
        }
    }

    /// Dönüşü bekleyen (`returned`) çağrıları çağırana işler — WFC-RETURN.
    pub async fn run_call_returns(&self, limit: i64) -> Result<usize, EngineError> {
        let pending = self.wfe.pending_call_returns(limit).await?;
        let mut applied = 0usize;
        for call in pending {
            match self.apply_one_return(&call).await {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        "WFC dönüşü uygulanamadı (call_row={}, caller={}): {e}",
                        call.id,
                        call.caller_wfe_id
                    );
                }
            }
        }
        Ok(applied)
    }

    async fn apply_one_return(&self, call: &PendingCall) -> Result<bool, EngineError> {
        let caller = self.wfe.load(call.caller_wfe_id).await?;
        // Çağıran artık o node'da beklemiyorsa (SLA devri, iptal, elle müdahale)
        // dönüş uygulanamaz — satır kapatılır, sessiz yanlış transition yapılmaz.
        let still_waiting = caller.status == WfeStatus::Active
            && caller.current_node.as_deref() == Some(call.site.key());
        if !still_waiting {
            self.wfe.set_call_status(call.id, "consumed", None).await?;
            return Ok(false);
        }
        let wfd = self.wfd.fetch(caller.wfd_id, caller.wfd_version).await?;
        let commit = self
            .engine()
            .fire_call_return(
                &wfd,
                &caller,
                call.call_status.as_deref().unwrap_or("completed"),
                call.callee_wfe_id,
                call.end_response.as_ref(),
                chrono::Utc::now(),
            )
            .await?;
        self.wfe.commit(&commit).await?;
        self.wfe.set_call_status(call.id, "consumed", None).await?;
        self.after_wfe_settled(caller.wfe_id, &commit.outcome)
            .await?;
        self.nudge_timers();
        Ok(true)
    }

    /// Süre sınırı geçmiş `wait` çağrıları — `$call.status = "timeout"` ile döner.
    pub async fn expire_overdue_calls(&self, limit: i64) -> Result<usize, EngineError> {
        let overdue = self.wfe.overdue_calls(chrono::Utc::now(), limit).await?;
        let mut n = 0usize;
        for call in overdue {
            // Çağrılan hâlâ koşuyorsa sonlandırılır; sonra çağıran "timeout" ile ilerler.
            if let Some(callee) = call.callee_wfe_id {
                if let Ok(callee_wfes) = self.wfe.load(callee).await {
                    if callee_wfes.status == WfeStatus::Active {
                        let commit = self
                            .engine()
                            .fire_deadline_timeout(&callee_wfes, chrono::Utc::now());
                        let _ = self.wfe.commit(&commit).await;
                    }
                }
            }
            let timed_out = PendingCall {
                call_status: Some("timeout".into()),
                end_response: None,
                ..call.clone()
            };
            if self.apply_one_return(&timed_out).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Bir WFE sonlandığında yapılacaklar: onu bekleyen çağrı satırını ilerlet ve
    /// kendi ALT AKIŞLARINI iptal et (WFC-CASCADE).
    ///
    /// **Ardıl KAPSAM DIŞI** — ardıl, astın aksine çağıranın ömrüne bağlı değildir;
    /// zaten çağıran bittikten sonra başlar (bkz. decisions.md → WFC).
    async fn after_wfe_settled(
        &self,
        wfe_id: Uuid,
        outcome: &CommitOutcome,
    ) -> Result<(), EngineError> {
        let (status, end_response) = match outcome {
            CommitOutcome::Terminal { end_response } => ("completed", Some(end_response)),
            CommitOutcome::Failed { end_response } => ("failed", Some(end_response)),
            CommitOutcome::Terminated { end_response } => ("terminated", Some(end_response)),
            CommitOutcome::JoinComplete { next, .. } => match next.as_ref() {
                CommitOutcome::Terminal { end_response } => ("completed", Some(end_response)),
                _ => return Ok(()),
            },
            _ => return Ok(()),
        };
        self.wfe
            .mark_callee_finished(wfe_id, status, end_response)
            .await?;
        // WFC-CASCADE: koşan alt akışları düşür.
        for callee in self.wfe.cancel_subcalls_of(wfe_id).await? {
            if let Ok(sub) = self.wfe.load(callee).await {
                if sub.status == WfeStatus::Active {
                    let commit = self
                        .engine()
                        .fire_deadline_timeout(&sub, chrono::Utc::now());
                    let _ = self.wfe.commit(&commit).await;
                }
            }
        }
        Ok(())
    }

    pub async fn tick_timers(&self, wfe_id: Uuid) -> Result<bool, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status != WfeStatus::Active {
            return Ok(false);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let now = chrono::Utc::now();
        let engine = self.engine();

        // SLA-3 deadline wfe-seviyesidir (paralel modda da) — tüm kolları iptal eder.
        if engine.deadline_due(&wfes, now) {
            let commit = engine.fire_deadline_timeout(&wfes, now);
            self.wfe.commit(&commit).await?;
            // SLA-3 ile sonlanma da bir sonlanmadır: bekleyen çağrı `terminated`
            // olarak döner (çağıran karar verir), alt akışlar iptal edilir.
            // Ardıl TETİKLENMEZ — bu başarılı bir bitiş değil (bkz. stage_calls).
            self.after_wfe_settled(wfe_id, &commit.outcome).await?;
            return Ok(true);
        }
        // WOR-31: paralel modda claim_timeout/escalation KOL-bazlıdır — aktif
        // kollar üzerinden iterasyon; ilk vadesi gelen ateşlenir. Paralel modda
        // değilse tek wfe-seviyesi kol (branch: None) mevcut davranışla taranır.
        let branches: Vec<Option<String>> = if wfes.join_target.is_some() {
            active_branch_nodes(&wfes).into_iter().map(Some).collect()
        } else {
            vec![None]
        };
        for branch in &branches {
            let b = branch.as_deref();
            if engine.claim_timeout_due(&wfd, &wfes, now, b)? {
                match engine.fire_claim_timeout(&wfd, &wfes, now, b).await? {
                    ClaimTimeoutOutcome::Move(commit) => {
                        self.wfe.commit(&commit).await?;
                        self.after_wfe_settled(wfe_id, &commit.outcome).await?;
                    }
                    ClaimTimeoutOutcome::Release(release) => {
                        self.wfe
                            .release_claim(
                                wfe_id,
                                wfes.orgtnt_id,
                                &release.wfah_entry,
                                b,
                                release.new_dynctx.as_ref(),
                            )
                            .await?
                    }
                }
                return Ok(true);
            }
            if let Some(idx) = engine.due_escalation(&wfd, &wfes, now, b)? {
                let commit = engine.fire_escalation(&wfd, &wfes, idx, now, b).await?;
                self.wfe.commit(&commit).await?;
                self.after_wfe_settled(wfe_id, &commit.outcome).await?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Tek WFE için en yakın SLA vadesi — `tick_timers`'ın izlediği üç sayacın
    /// (SLA-3 deadline, SLA-1 claim_timeout, SLA-2 sıradaki escalation) min'i.
    /// Salt-okunur; hiçbir şey ateşlemez. Aktif değilse `None`. Timer servisinin
    /// (bkz. `crate::timer`) uyku süresi hesabı için kullanılır.
    pub async fn next_timer_due(
        &self,
        wfe_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<DateTime<Utc>>, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status != WfeStatus::Active {
            return Ok(None);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        // SLA-3 deadline wfe-seviyesi taban; claim_timeout + escalation paralel
        // modda KOL-bazlı (her aktif kol için ayrı sayaç), aksi halde wfe-seviyesi.
        let mut due = wfes.deadline;
        let mut fold = |cand: Option<DateTime<Utc>>| {
            if let Some(c) = cand {
                due = Some(match due {
                    Some(d) => d.min(c),
                    None => c,
                });
            }
        };

        if wfes.join_target.is_some() {
            for b in &wfes.branches {
                if b.status != BranchStatus::Active {
                    continue;
                }
                fold(compute_claim_deadline(
                    &wfd,
                    Some(&b.branch_node),
                    b.claimed_at,
                ));
                fold(
                    self.engine()
                        .next_escalation(&wfd, &wfes, now, Some(&b.branch_node))?
                        .map(|f| f.deadline),
                );
            }
        } else {
            fold(compute_claim_deadline(
                &wfd,
                wfes.current_node.as_deref(),
                wfes.claimed_at,
            ));
            fold(
                self.engine()
                    .next_escalation(&wfd, &wfes, now, None)?
                    .map(|f| f.deadline),
            );
        }
        Ok(due)
    }

    /// Tek WFE için bir sonraki escalation vadesi — dashboard insight'ı
    /// (yaklaşan/geciken escalation'lar). `tick_timers`'ın salt-okunur hâli:
    /// hiçbir şey ateşlemez/commit etmez.
    pub async fn escalation_forecast(
        &self,
        wfe_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<wfe_core::v22::pipeline::EscalationForecast>, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status != WfeStatus::Active {
            return Ok(None);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        self.engine().next_escalation(&wfd, &wfes, now, None)
    }
}
