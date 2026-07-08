//! v2.2 WfeExecutor — saf Engine (wfe-core::v22::pipeline) ile store'lar arasındaki
//! ince orkestrasyon katmanı. Tüm yazımlar WfeStore'un atomik create/commit/claim'i
//! üzerinden gider (M8).

use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;
use wfe_core::types::actor::{Actor, CandidateActor};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::matcher::MatchEnv;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::v22::ports::{AutoexecRunner, CommitOutcome, WfdStore, WfeStore};
use wfe_core::v22::visibility::{can_view, filter_dynctx};
use wfe_core::{EngineError, OrgPort};

pub struct WfeExecutor {
    pub org: Arc<dyn OrgPort>,
    pub wfd: Arc<dyn WfdStore>,
    pub wfe: Arc<dyn WfeStore>,
    pub runner: Arc<dyn AutoexecRunner>,
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
        Self { org, wfd, wfe, runner }
    }

    fn engine(&self) -> Engine<'_> {
        Engine {
            org: &*self.org,
            exec: &*self.runner,
        }
    }

    pub async fn start(
        &self,
        wfd_id: Uuid,
        version: i32,
        actor: &Actor,
        input: &Value,
    ) -> Result<WfeStartResult, EngineError> {
        let wfd = self.wfd.fetch(wfd_id, version).await?;
        let orgtnt_id = self.org.orgtnt_for_orgu(actor.orgu_id).await?;

        // wfe_id ÖNCE üretilir; $wfe_id effects gerçek id ile çözülür (WOR-6)
        let wfe_id = Uuid::new_v4();
        let mut new = self
            .engine()
            .start(&wfd, actor, orgtnt_id, input, wfe_id)
            .await?;
        new.wfd_id = wfd_id;
        new.wfd_version = version;

        self.wfe.create(&new).await?;

        let (terminal, current_node, end_response) = match &new.outcome {
            CommitOutcome::MoveTo { node } => (false, Some(node.clone()), None),
            CommitOutcome::Terminal { end_response } => (true, None, Some(end_response.clone())),
            CommitOutcome::Failed { end_response } => (true, None, Some(end_response.clone())),
        };
        Ok(WfeStartResult {
            wfe_id,
            terminal,
            current_node,
            end_response,
            current_c_a: new.resolved_c_a,
        })
    }

    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
        action: &str,
        input: &Value,
    ) -> Result<WfeApplyResult, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        let commit = self.engine().apply(&wfd, &wfes, actor, action, input).await?;
        self.wfe.commit(&commit).await?;

        let (terminal, current_node, end_response) = match &commit.outcome {
            CommitOutcome::MoveTo { node } => (false, Some(node.clone()), None),
            CommitOutcome::Terminal { end_response } => (true, None, Some(end_response.clone())),
            CommitOutcome::Failed { end_response } => (true, None, Some(end_response.clone())),
        };
        Ok(WfeApplyResult {
            wfe_id,
            terminal,
            current_node,
            end_response,
            current_c_a: commit.resolved_c_a,
        })
    }

    /// Claim uygunluğu — matcher tabanlı (§7.1); c_u kuralları dahil doğru çalışır.
    pub async fn can_claim(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
    ) -> Result<(bool, Option<String>), EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.assigned_to == Some(actor.user_id) {
            return Ok((true, None));
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        Ok(match self.engine().can_claim(&wfd, &wfes, actor).await? {
            ClaimCheck::Ok => (true, None),
            ClaimCheck::AlreadyClaimed => (false, Some("already_claimed".into())),
            ClaimCheck::Terminal => (false, Some("terminal".into())),
            ClaimCheck::NotEligible => (false, Some("not_eligible".into())),
        })
    }

    /// Atomik claim: uygunluk matcher ile doğrulanır, yazım CAS ile yapılır.
    pub async fn claim(&self, wfe_id: Uuid, actor: &Actor) -> Result<ClaimOutcome, EngineError> {
        let (eligible, reason) = self.can_claim(wfe_id, actor).await?;
        if !eligible {
            return Ok(ClaimOutcome { success: false, reason });
        }
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.assigned_to == Some(actor.user_id) {
            return Ok(ClaimOutcome { success: true, reason: None });
        }
        let won = self
            .wfe
            .claim(wfe_id, wfes.orgtnt_id, actor.user_id)
            .await?;
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

        Ok(WfeView {
            wfe_id,
            status: wfes.status,
            current_node: wfes.current_node,
            claimed_by: wfes.assigned_to,
            dynctx: filtered,
            wfah: wfes.wfah.entries().to_vec(),
            end_response: wfes.end_response,
        })
    }

    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
    ) -> Result<Vec<String>, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status == WfeStatus::Terminal {
            return Ok(vec![]);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        self.engine().possible_actions(&wfd, &wfes, actor).await
    }

    /// Tek WFE için zamanlayıcı kontrolü: önce root timeout, sonra escalation.
    /// Bir şey ateşlendiyse true döner (M5/M6 — WOR-46/47).
    pub async fn tick_timers(&self, wfe_id: Uuid) -> Result<bool, EngineError> {
        let wfes = self.wfe.load(wfe_id).await?;
        if wfes.status != WfeStatus::Active {
            return Ok(false);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let now = chrono::Utc::now();
        let engine = self.engine();

        if engine.root_timeout_due(&wfd, &wfes, now)? {
            let commit = engine.fire_root_timeout(&wfd, &wfes, now)?;
            self.wfe.commit(&commit).await?;
            return Ok(true);
        }
        if let Some(idx) = engine.due_escalation(&wfd, &wfes, now)? {
            let commit = engine.fire_escalation(&wfd, &wfes, idx, now).await?;
            self.wfe.commit(&commit).await?;
            return Ok(true);
        }
        Ok(false)
    }
}
