//! v2.2 simülasyon durumu — editörün simulate endpoint'leri için.
//! Engine saf olduğundan store gerekmez: SimState ⇄ Wfes dönüşümü ve
//! TransitionCommit'in in-memory uygulanması yeterlidir.
//! Not: simülasyonda claim akışı atlanır — apply öncesi state aktöre atanır.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wfe_core::types::{
    actor::Actor,
    dynctx::DynCtx,
    wfah::{Wfah, WfahEntry},
    wfd_v22::{JoinRule, WftTarget},
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{
    BranchState, BranchStatus, CommitOutcome, NewWfe, StagedCall, TransitionCommit, Wfes,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimState {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub dynctx: serde_json::Value,
    pub wfah: Vec<WfahEntry>,
    pub current_node: Option<String>,
    pub status: WfeStatus,
    pub end_response: Option<serde_json::Value>,
    /// WOR-31 T4: paralel mod kol durumları (JSON alan adı `node`, bkz.
    /// `BranchState`); paralel modda değilken boş. `#[serde(default)]` — eski
    /// (fork öncesi üretilmiş) sim_state blob'ları bu alan olmadan da parse edilir.
    #[serde(default)]
    pub branches: Vec<BranchState>,
    /// WOR-31 T4: fork'ta persist edilen join hedefi; `Some` = paralel mod.
    #[serde(default)]
    pub join_target: Option<WftTarget>,
    /// WOR-72: fork'ta persist edilen quorum eşiği; `None` = AND-join (tüm kollar),
    /// `Some(k)` = k varış yeterli. `#[serde(default)]` — WOR-72 öncesi üretilmiş
    /// sim_state blob'ları bu alan olmadan parse edilir (AND olarak okunur).
    #[serde(default)]
    pub join_threshold: Option<u32>,
    /// WOR-73: fork'ta persist edilen ZEN join koşulu (`join_mode: expr`); `None` =
    /// eşik/AND kuralı. Eşikle birlikte DOLU OLMAZ (bkz. `join_rule`).
    #[serde(default)]
    pub join_when: Option<String>,
    /// WFC: bu adımda yapılacak iş akışı çağrıları.
    ///
    /// Simülasyonda GERÇEK bir WFE yaratılmaz — çağrılan akışı koşturmak simülasyonun
    /// kapsamı değildir (kendi aktörleri, kendi SLA'sı, kendi org çözümü olurdu).
    /// Onun yerine çağrı burada "bekliyor" olarak durur ve kullanıcı sonucu ELLE girer
    /// (`/simulate/call-return`). Editör bu listeyi görüp "burada şu akış çağrılacak"
    /// diyebilir; `wait` modunda akış bu çağrı çözülene kadar ilerlemez.
    ///
    /// `#[serde(default)]` — WFC öncesi üretilmiş sim_state blob'ları bu alan olmadan
    /// da parse edilir.
    #[serde(default)]
    pub pending_calls: Vec<SimCall>,
}

/// Simülasyonda bekleyen bir WFC çağrısı — `wf.wfe_call`'ın kullanıcıya görünen özeti.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimCall {
    /// Root `calls` katalog key'i.
    pub call_key: String,
    /// wait | detached | terminal
    pub mode: String,
    /// 'node' (alt akış) | 'terminal' (ardıl akış)
    pub site_kind: String,
    /// Çağrı node'unun slug'ı ya da terminal id'si.
    pub site_key: String,
    /// WFC-IN — çağıranın ctx'inden ÇÖZÜLMÜŞ girdi. Editör bunu göstererek
    /// "çağrılana şu değerler gidecek" diyebilir.
    pub input: serde_json::Value,
    /// `wait` dışındaki modlarda çağrı beklenmez: satır bilgi amaçlıdır ve
    /// akışı bloklamaz (`detached` hemen devam eder, `terminal`'de akış bitmiştir).
    pub awaited: bool,
}

impl SimCall {
    fn from_staged(c: &StagedCall) -> Self {
        let awaited = c.mode == wfe_core::types::wfd_v22::CallMode::Wait;
        Self {
            call_key: c.call_key.clone(),
            mode: c.mode.as_str().into(),
            site_kind: c.site.kind().into(),
            site_key: c.site.key().into(),
            input: c.input.clone(),
            awaited,
        }
    }
}

impl SimState {
    /// `start`'ın wft'i Parallel OLAMAZ (validator + engine reddeder — WOR-31
    /// §Kısıtlar), bu yüzden `branches`/`join_target` her zaman boş başlar.
    pub fn from_new_wfe(new: &NewWfe) -> Self {
        let (status, current_node, end_response) = outcome_parts(&new.outcome);
        Self {
            wfe_id: new.wfe_id,
            orgtnt_id: new.orgtnt_id,
            dynctx: new.initial_dynctx.clone(),
            wfah: new.wfah_entries.clone(),
            current_node,
            status,
            end_response,
            branches: vec![],
            join_target: None,
            join_threshold: None,
            join_when: None,
            pending_calls: new.staged_calls.iter().map(SimCall::from_staged).collect(),
        }
    }

    /// Simülasyon Wfes'i — `assigned_to` verilen aktöre bağlanır (claim bypass).
    /// WOR-31 T4: paralel modda `/simulate` altında ayrı bir `/claim` endpoint'i
    /// YOK (routes/simulate.rs yalnızca start/apply/possible-actions taşır) —
    /// bypass paralel modda da aynı ruhla genişletilir: verilen aktör TÜM aktif
    /// kolların `claimed_by`'ı sayılır (o kolun c_a'sına uygun olup olmadığına
    /// bakılmaksızın — tek-node bypass'ın zaten yaptığı basitleştirmenin
    /// birebir kol-seviyesi karşılığı). Simülasyonda SLA alanları izlenmez:
    /// deadline/claimed_at her zaman NULL.
    pub fn to_wfes(&self, assigned_to: Option<Uuid>) -> Wfes {
        let created_at = self
            .wfah
            .first()
            .map(|e| e.applied_at)
            .unwrap_or_else(chrono::Utc::now);
        let branches = match assigned_to {
            Some(uid) => self
                .branches
                .iter()
                .cloned()
                .map(|mut b| {
                    if b.status == BranchStatus::Active {
                        b.claimed_by = Some(uid);
                    }
                    b
                })
                .collect(),
            None => self.branches.clone(),
        };
        Wfes {
            wfe_id: self.wfe_id,
            orgtnt_id: self.orgtnt_id,
            environment_id: None,
            wfd_id: Uuid::nil(),
            wfd_version: 0,
            dynctx: DynCtx(self.dynctx.clone()),
            wfah: Wfah(self.wfah.clone()),
            status: self.status.clone(),
            current_node: self.current_node.clone(),
            assigned_to,
            end_response: self.end_response.clone(),
            deadline: None,
            claimed_at: None,
            created_at,
            branches,
            join_target: self.join_target.clone(),
            // WOR-72/WOR-73: iki alan → tek çözülmüş kural (adapter'daki okuma ile
            // aynı sıra: eşik önce).
            join_rule: match (self.join_threshold, self.join_when.clone()) {
                (Some(k), _) => JoinRule::Quorum(k.max(1)),
                (None, Some(expr)) => JoinRule::Expr(expr),
                (None, None) => JoinRule::All,
            },
        }
    }

    pub fn apply_commit(&mut self, commit: &TransitionCommit) {
        self.dynctx = commit.new_dynctx.clone();
        self.wfah.extend(commit.wfah_entries.iter().cloned());
        self.apply_branch_outcome(&commit.outcome);
        let (status, current_node, end_response) = outcome_parts(&commit.outcome);
        self.status = status;
        self.current_node = current_node;
        if end_response.is_some() {
            self.end_response = end_response;
        }
        // WFC: yeni çağrılar eklenir. Öncekiler KORUNUR — `wait` modunda akış zaten
        // çağrı çözülmeden ilerleyemez; `detached`/`terminal` satırları ise geçmişin
        // parçasıdır ve editörde "şu adımda şu akış başlatıldı" olarak kalır.
        self.pending_calls
            .extend(commit.staged_calls.iter().map(SimCall::from_staged));
    }

    /// WFC: bu adımda çözülmesi BEKLENEN çağrı (yalnız `mode: wait`).
    ///
    /// `Some` ise akış bu node'da duruyor ve ilerlemesi için çağrı sonucunun elle
    /// girilmesi gerekir — editör "sonucu gir" formunu bu bilgiyle açar.
    pub fn awaited_call(&self) -> Option<&SimCall> {
        let node = self.current_node.as_deref()?;
        self.pending_calls
            .iter()
            .find(|c| c.awaited && c.site_kind == "node" && c.site_key == node)
    }

    /// Çağrı çözüldükten sonra satırı bekleyen listeden düşürür (dönüş bir kez işlenir).
    pub fn clear_awaited_call(&mut self, site_key: &str) {
        self.pending_calls
            .retain(|c| !(c.awaited && c.site_kind == "node" && c.site_key == site_key));
    }

    /// WOR-31 T4: `branches`/`join_target` yan etkileri — DB adapter'ının commit
    /// mantığının (bkz. `wfe_adapter.rs`) in-memory karşılığı. Sim TEK-THREAD'li
    /// çalıştığından eşzamanlı varış yarışı yoktur: adapter'ın `FOR UPDATE` +
    /// kol-CAS + aktif-kol sayımı doğrulaması burada YOKTUR — engine'in
    /// BranchArrived/JoinComplete görüşü sorgusuz doğru kabul edilir (T4
    /// sözleşmesi: "JoinComplete verify trivially true", Conflict imkansız).
    fn apply_branch_outcome(&mut self, outcome: &CommitOutcome) {
        match outcome {
            CommitOutcome::ForkTo {
                branches,
                join,
                join_rule,
            } => {
                self.join_target = Some(join.clone());
                let (threshold, when) = match join_rule {
                    JoinRule::All => (None, None),
                    JoinRule::Quorum(k) => (Some(*k), None),
                    JoinRule::Expr(e) => (None, Some(e.clone())),
                };
                self.join_threshold = threshold;
                self.join_when = when;
                let now = chrono::Utc::now();
                self.branches = branches
                    .iter()
                    .map(|n| BranchState {
                        branch_node: n.clone(),
                        // WOR-73: kol kimliği = giriş node'u, bir daha değişmez.
                        entry_node: n.clone(),
                        status: BranchStatus::Active,
                        claimed_by: None,
                        claimed_at: None,
                        entered_at: now,
                    })
                    .collect();
            }
            CommitOutcome::BranchMoveTo { from_node, node } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.branch_node = node.clone();
                    b.claimed_by = None;
                    b.claimed_at = None;
                    b.entered_at = chrono::Utc::now();
                }
            }
            CommitOutcome::BranchArrived { from_node, .. } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.status = BranchStatus::Arrived;
                    b.claimed_by = None;
                    b.claimed_at = None;
                }
            }
            CommitOutcome::JoinComplete { from_node, .. } => {
                if let Some(b) = self.active_branch_mut(from_node) {
                    b.status = BranchStatus::Arrived;
                }
                // `_join` — dokümante istisna: engine DEĞİL, DB'de adapter
                // (`wfe_adapter.rs`) aynı tx'te ekler; sim'de adapter yok, o
                // yüzden burada birebir taklit edilir (seq = son staged + 1).
                let seq = self.wfah.last().map(|e| e.seq + 1).unwrap_or(1);
                self.wfah.push(WfahEntry {
                    seq,
                    action: "_join".into(),
                    actor: Actor {
                        orgu_id: Uuid::nil(),
                        user_id: Uuid::nil(),
                        role: "system".into(),
                    },
                    input: None,
                    applied_at: chrono::Utc::now(),
                });
                // Join doldu — paralel mod biter; kollar (DB'nin aksine, audit
                // amacıyla) sim'de basitçe temizlenir. WOR-72: quorum modunda
                // geride kalan aktif kollar iptal edilmiş sayılır (marker'lar
                // engine'de staged edildi) — sim'de ayrı işaretleme gerekmez.
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            CommitOutcome::Terminal { .. }
            | CommitOutcome::Failed { .. }
            | CommitOutcome::Terminated { .. } => {
                // WFE tümden bitti — paralel modda aktif kollar iptal edilmiş
                // sayılır (`_branch_cancelled` marker'ları engine tarafından
                // zaten wfah_entries'e staged edildi); sim'de satırlar tutulmaz.
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            // WOR-56: node hedefli collapse — paralel mod biter (kardeşler iptal,
            // marker'lar engine'de staged), WFE `node`'a geçer (current_node
            // outcome_parts'ta set edilir); kol satırları sim'de temizlenir.
            CommitOutcome::CollapseTo { .. } => {
                self.join_target = None;
                self.join_threshold = None;
                self.join_when = None;
                self.branches.clear();
            }
            CommitOutcome::MoveTo { .. } => {}
        }
    }

    fn active_branch_mut(&mut self, node: &str) -> Option<&mut BranchState> {
        self.branches
            .iter_mut()
            .find(|b| b.status == BranchStatus::Active && b.branch_node == node)
    }
}

fn outcome_parts(
    outcome: &CommitOutcome,
) -> (WfeStatus, Option<String>, Option<serde_json::Value>) {
    match outcome {
        CommitOutcome::MoveTo { node } => (WfeStatus::Active, Some(node.clone()), None),
        CommitOutcome::Terminal { end_response } => {
            (WfeStatus::Terminal, None, Some(end_response.clone()))
        }
        CommitOutcome::Failed { end_response } => {
            (WfeStatus::Error, None, Some(end_response.clone()))
        }
        CommitOutcome::Terminated { end_response } => {
            (WfeStatus::Terminated, None, Some(end_response.clone()))
        }
        // WOR-31: paralel outcome'lar aktiftir, wfe-seviyesi current_node
        // taşımaz; kol durumunun sim'e işlenmesi T4'te (SimState.branches).
        CommitOutcome::ForkTo { .. }
        | CommitOutcome::BranchMoveTo { .. }
        | CommitOutcome::BranchArrived { .. } => (WfeStatus::Active, None, None),
        CommitOutcome::JoinComplete { next, .. } => outcome_parts(next),
        // WOR-56: node hedefli collapse — paralel mod biter, WFE tekil modda `node`'a.
        CommitOutcome::CollapseTo { node, .. } => (WfeStatus::Active, Some(node.clone()), None),
    }
}
