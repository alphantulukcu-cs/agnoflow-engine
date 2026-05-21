use std::sync::Arc;
use serde_json::Value;
use uuid::Uuid;
use wfe_core::{
    engine::{
        c_a_resolver::resolve_c_a,
        transition::{apply_action, WftOutcome},
        visibility,
    },
    error::EngineError,
    ports::{OrgPort, WfdPort, WfePort, WFES},
    types::{
        actor::{Actor, CandidateActor, COrguExpr, CaRule},
        dynctx::DynCtx,
        wfah::Wfah,
        wfd::WftRule,
        wfe::WfeStatus,
    },
    zen,
};

pub struct WfeExecutor {
    pub org: Arc<dyn OrgPort>,
    pub wfd: Arc<dyn WfdPort>,
    pub wfe: Arc<dyn WfePort>,
}

impl WfeExecutor {
    pub fn new(
        org: Arc<dyn OrgPort>,
        wfd: Arc<dyn WfdPort>,
        wfe: Arc<dyn WfePort>,
    ) -> Self {
        Self { org, wfd, wfe }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct WfeStartResult {
    pub wfe_id:      Uuid,
    pub current_c_a: Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeApplyResult {
    pub wfe_id:       Uuid,
    pub terminal:     bool,
    pub end_response: Option<Value>,
    pub current_c_a:  Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeView {
    pub wfe_id:       Uuid,
    pub status:       WfeStatus,
    pub dynctx:       Value,
    pub wfah:         Vec<wfe_core::types::wfah::WfahEntry>,
    pub current_c_a:  Value,
    pub end_response: Option<Value>,
}

impl WfeExecutor {
    /// Start a new WFE instance.
    pub async fn start(
        &self,
        wfd_id:  Uuid,
        version: u32,
        actor:   &Actor,
        input:   &Value,
    ) -> Result<WfeStartResult, EngineError> {
        let wfd = self.wfd.fetch(wfd_id, version).await?;

        // Find a start rule the actor is eligible for
        let start_rule = 'outer: {
            for rule in &wfd.start {
                for ca_rule in &rule.c_a {
                    let expr = ca_orgu_expr(ca_rule);
                    let orgus = self.org
                        .resolve_c_orgu(actor.orgu_id, &expr, actor.orgu_id)
                        .await?;
                    let in_orgu = orgus.iter().any(|u| u.orgu_id == actor.orgu_id);
                    if in_orgu && self.org.check_user_role(actor.user_id, actor.orgu_id, &actor.role).await? {
                        break 'outer Some(rule);
                    }
                }
            }
            None
        };

        let start_rule = start_rule.ok_or(EngineError::StartNotEligible)?;

        let temp_wfe_id = Uuid::new_v4();
        let initial_dynctx = wfe_core::engine::dynctx_apply::apply(
            &DynCtx::empty(), &start_rule.wfes_effects, actor, temp_wfe_id, "start", input
        )?;

        let temp_wfes = WFES {
            wfe_id:       temp_wfe_id,
            dynctx:       initial_dynctx.clone(),
            wfah:         Wfah::empty(),
            status:       WfeStatus::Active,
            orgtnt_id:    actor.orgu_id, // placeholder; real orgtnt resolved by org adapter
            wfd_id,
            wfd_version:  version,
            current_c_a:  vec![],
            end_response: None,
        };
        let initial_c_a = resolve_wft_c_a(&start_rule.wft, &temp_wfes, actor.orgu_id, &*self.org).await?;

        let wfe_id = self.wfe.create_wfe(
            actor.orgu_id, wfd_id, version, &initial_dynctx, &initial_c_a,
        ).await?;

        let entry = wfe_core::types::wfah::WfahEntry {
            seq:        1,
            action:     "start".into(),
            actor:      actor.clone(),
            input:      Some(input.clone()),
            applied_at: chrono::Utc::now(),
        };
        self.wfe.append_wfah(wfe_id, &entry).await?;

        Ok(WfeStartResult { wfe_id, current_c_a: initial_c_a })
    }

    /// Apply an action to an existing WFE.
    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
        action: &str,
        input:  &Value,
    ) -> Result<WfeApplyResult, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;

        if wfes.status == WfeStatus::Terminal {
            return Err(EngineError::WfeTerminal);
        }

        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let (new_wfes, outcome) = apply_action(&wfes, actor, action, input, &wfd, &*self.org).await?;

        let dynctx_seq = new_wfes.wfah.entries().len() as u32;

        self.wfe.persist_new_dynctx(wfe_id, &new_wfes.dynctx, dynctx_seq).await?;

        if let Some(entry) = new_wfes.wfah.entries().last() {
            self.wfe.append_wfah(wfe_id, entry).await?;
        }

        match outcome {
            WftOutcome::Terminal { end_response } => {
                self.wfe.set_terminal(wfe_id, &end_response).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal:     true,
                    end_response: Some(end_response),
                    current_c_a:  vec![],
                })
            }
            WftOutcome::NextCa(c_a) => {
                self.wfe.update_c_a(wfe_id, &c_a).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal:     false,
                    end_response: None,
                    current_c_a:  c_a,
                })
            }
        }
    }

    /// Query WFE state with visibility filtering applied.
    pub async fn query(&self, wfe_id: Uuid, viewer: &Actor) -> Result<WfeView, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        let wfd  = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let filtered_ctx = visibility::apply(&wfes.dynctx, viewer, &wfd);

        Ok(WfeView {
            wfe_id,
            status:       wfes.status.clone(),
            dynctx:       filtered_ctx,
            wfah:         wfes.wfah.entries().to_vec(),
            current_c_a:  serde_json::to_value(&wfes.current_c_a).unwrap_or_default(),
            end_response: wfes.end_response.clone(),
        })
    }

    /// Returns action names the actor can perform on this WFE right now.
    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor:  &Actor,
    ) -> Result<Vec<String>, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        if wfes.status == WfeStatus::Terminal {
            return Ok(vec![]);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        let mut actions = Vec::new();
        for t in &wfd.transitions {
            if zen::evaluate(&t.when, &wfes)? {
                if wfe_core::engine::c_a_resolver::actor_in_c_a(
                    &t.c_a, actor, wfes.orgtnt_id, &*self.org
                ).await? {
                    actions.push(t.action.clone());
                }
            }
        }
        actions.dedup();
        Ok(actions)
    }
}

fn ca_orgu_expr(rule: &CaRule) -> String {
    match &rule.c_orgu {
        COrguExpr::Expr(s)               => s.clone(),
        COrguExpr::Anchored { traverse, .. } => traverse.clone(),
    }
}

async fn resolve_wft_c_a(
    wft:    &WftRule,
    wfes:   &WFES,
    anchor: Uuid,
    org:    &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    match wft {
        WftRule::Simple { c_a } => {
            resolve_c_a(c_a, anchor, wfes.orgtnt_id, org).await
        }
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if !cond.terminal {
                    if let Some(c_a) = &cond.c_a {
                        return resolve_c_a(c_a, anchor, wfes.orgtnt_id, org).await;
                    }
                }
            }
            Ok(vec![])
        }
    }
}
