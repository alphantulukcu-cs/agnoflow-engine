//! v2.2 WfeExecutor — saf Engine (wfe-core::v22::pipeline) ile store'lar arasındaki
//! ince orkestrasyon katmanı. Tüm yazımlar WfeStore'un atomik create/commit/claim'i
//! üzerinden gider (M8).

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;
use wfe_core::types::actor::{Actor, CandidateActor};
use wfe_core::types::wfd_v22::{Wfd, WftTarget};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::matcher::MatchEnv;
use wfe_core::v22::pipeline::{ClaimCheck, ClaimTimeoutOutcome, Engine};
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CommitOutcome, WfdStore, WfeStore, Wfes,
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
        Some(n) => wfes
            .branches
            .iter()
            .find(|b| b.status == BranchStatus::Active && b.branch_node == n)
            .and_then(|b| b.claimed_by)
            == Some(user_id),
        None => wfes.assigned_to == Some(user_id),
    }
}

/// WOR-31: WFE'nin şu an aktif kollarının node adları (paralel timer taraması
/// bunlar üzerinden döner). Paralel modda değilse boş.
fn active_branch_nodes(wfes: &Wfes) -> Vec<String> {
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
            let actions = engine.possible_actions(wfd, wfes, actor, Some(&node)).await?;
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
    /// JSON alan adı `node` (bkz. `BranchState`).
    pub branches: Vec<BranchState>,
    /// WOR-31 T4: fork'ta persist edilen AND-join hedefi; `Some` = paralel mod
    /// (bu durumda `current_node` `None`'dur).
    pub join_target: Option<WftTarget>,
    /// WOR-65: WFE revizyon token'ı (bkz. `Wfes::rev()`). İstemci bunu okuyup
    /// bir sonraki apply/claim'de `expected_rev` olarak geri gönderirse, arada
    /// durum değişmişse 409 `conflict.stale_revision` alır. WFE-seviyesidir —
    /// paralel modda TÜM kollar için aynı değerdir.
    pub rev: u32,
}

#[derive(Debug, serde::Serialize)]
pub struct ClaimOutcome {
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
                    self.nudge_timers(); // node değişimi escalation dwell'ini ve claim sayacını sıfırlar
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
        Ok(match self.engine().can_claim(&wfd, &wfes, actor, node).await? {
            ClaimCheck::Ok => (true, None),
            ClaimCheck::AlreadyClaimed => (false, Some("already_claimed".into())),
            ClaimCheck::Terminal => (false, Some("terminal".into())),
            ClaimCheck::Expired => (false, Some("expired".into())),
            ClaimCheck::NotEligible => (false, Some("not_eligible".into())),
        })
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
            return Ok(ClaimOutcome { success: false, reason });
        }
        let wfes = self.wfe.load(wfe_id).await?;
        if branch_owner_is(&wfes, node, actor.user_id) {
            return Ok(ClaimOutcome { success: true, reason: None });
        }
        let won = self
            .wfe
            .claim(wfe_id, wfes.orgtnt_id, actor.user_id, node)
            .await?;
        if won {
            self.nudge_timers(); // claim_timeout sayacı şimdi başladı (SLA-1)
        }
        Ok(ClaimOutcome {
            success: won,
            reason: if won { None } else { Some("already_claimed".into()) },
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
            branches: wfes.branches,
            join_target: wfes.join_target,
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
    /// 1. instance deadline (SLA-3), 2. claim timeout (SLA-1), 3. escalation
    /// (SLA-2, `terminate:true` dahil). Bir şey ateşlendiyse true döner
    /// (M5/M6 — WOR-46/47).
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
                    ClaimTimeoutOutcome::Move(commit) => self.wfe.commit(&commit).await?,
                    ClaimTimeoutOutcome::Release(entry) => {
                        self.wfe.release_claim(wfe_id, wfes.orgtnt_id, &entry, b).await?
                    }
                }
                return Ok(true);
            }
            if let Some(idx) = engine.due_escalation(&wfd, &wfes, now, b)? {
                let commit = engine.fire_escalation(&wfd, &wfes, idx, now, b).await?;
                self.wfe.commit(&commit).await?;
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
                fold(compute_claim_deadline(&wfd, Some(&b.branch_node), b.claimed_at));
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
