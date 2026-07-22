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
    wfd_v22::WftTarget,
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{
    BranchState, BranchStatus, CommitOutcome, NewWfe, TransitionCommit, Wfes,
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
    /// WOR-31 T4: fork'ta persist edilen AND-join hedefi; `Some` = paralel mod.
    #[serde(default)]
    pub join_target: Option<WftTarget>,
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
    }

    /// WOR-31 T4: `branches`/`join_target` yan etkileri — DB adapter'ının commit
    /// mantığının (bkz. `wfe_adapter.rs`) in-memory karşılığı. Sim TEK-THREAD'li
    /// çalıştığından eşzamanlı varış yarışı yoktur: adapter'ın `FOR UPDATE` +
    /// kol-CAS + aktif-kol sayımı doğrulaması burada YOKTUR — engine'in
    /// BranchArrived/JoinComplete görüşü sorgusuz doğru kabul edilir (T4
    /// sözleşmesi: "JoinComplete verify trivially true", Conflict imkansız).
    fn apply_branch_outcome(&mut self, outcome: &CommitOutcome) {
        match outcome {
            CommitOutcome::ForkTo { branches, join } => {
                self.join_target = Some(join.clone());
                let now = chrono::Utc::now();
                self.branches = branches
                    .iter()
                    .map(|n| BranchState {
                        branch_node: n.clone(),
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
            CommitOutcome::BranchArrived { from_node } => {
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
                // Son varış — paralel mod biter; kollar (DB'nin aksine, audit
                // amacıyla) sim'de basitçe temizlenir.
                self.join_target = None;
                self.branches.clear();
            }
            CommitOutcome::Terminal { .. }
            | CommitOutcome::Failed { .. }
            | CommitOutcome::Terminated { .. } => {
                // WFE tümden bitti — paralel modda aktif kollar iptal edilmiş
                // sayılır (`_branch_cancelled` marker'ları engine tarafından
                // zaten wfah_entries'e staged edildi); sim'de satırlar tutulmaz.
                self.join_target = None;
                self.branches.clear();
            }
            // WOR-56: node hedefli collapse — paralel mod biter (kardeşler iptal,
            // marker'lar engine'de staged), WFE `node`'a geçer (current_node
            // outcome_parts'ta set edilir); kol satırları sim'de temizlenir.
            CommitOutcome::CollapseTo { .. } => {
                self.join_target = None;
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
