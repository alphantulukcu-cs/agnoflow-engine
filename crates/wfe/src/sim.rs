//! v2.2 simülasyon durumu — editörün simulate endpoint'leri için.
//! Engine saf olduğundan store gerekmez: SimState ⇄ Wfes dönüşümü ve
//! TransitionCommit'in in-memory uygulanması yeterlidir.
//! Not: simülasyonda claim akışı atlanır — apply öncesi state aktöre atanır.

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wfe_core::types::{
    dynctx::DynCtx,
    wfah::{Wfah, WfahEntry},
    wfe::WfeStatus,
};
use wfe_core::v22::ports::{CommitOutcome, NewWfe, TransitionCommit, Wfes};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimState {
    pub wfe_id: Uuid,
    pub orgtnt_id: Uuid,
    pub dynctx: serde_json::Value,
    pub wfah: Vec<WfahEntry>,
    pub current_node: Option<String>,
    pub status: WfeStatus,
    pub end_response: Option<serde_json::Value>,
}

impl SimState {
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
        }
    }

    /// Simülasyon Wfes'i — `assigned_to` verilen aktöre bağlanır (claim bypass).
    /// Simülasyonda SLA alanları izlenmez: deadline/claimed_at her zaman NULL.
    pub fn to_wfes(&self, assigned_to: Option<Uuid>) -> Wfes {
        let created_at = self
            .wfah
            .first()
            .map(|e| e.applied_at)
            .unwrap_or_else(chrono::Utc::now);
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
        }
    }

    pub fn apply_commit(&mut self, commit: &TransitionCommit) {
        self.dynctx = commit.new_dynctx.clone();
        self.wfah.extend(commit.wfah_entries.iter().cloned());
        let (status, current_node, end_response) = outcome_parts(&commit.outcome);
        self.status = status;
        self.current_node = current_node;
        if end_response.is_some() {
            self.end_response = end_response;
        }
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
    }
}
