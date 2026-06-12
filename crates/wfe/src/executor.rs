use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;
use wfe_core::{
    engine::{
        c_a_resolver::resolve_c_a,
        dynctx_apply,
        transition::{apply_action, WftOutcome},
        visibility,
    },
    error::EngineError,
    ports::{OrgPort, WfdPort, WfePort, WFES},
    types::{
        actor::{Actor, CandidateActor},
        dynctx::DynCtx,
        wfah::Wfah,
        wfd::{WFD, WftRule},
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
    pub fn new(org: Arc<dyn OrgPort>, wfd: Arc<dyn WfdPort>, wfe: Arc<dyn WfePort>) -> Self {
        Self { org, wfd, wfe }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct WfeStartResult {
    pub wfe_id: Uuid,
    pub current_c_a: Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeApplyResult {
    pub wfe_id: Uuid,
    pub terminal: bool,
    pub end_response: Option<Value>,
    pub current_c_a: Vec<CandidateActor>,
}

#[derive(Debug, serde::Serialize)]
pub struct WfeView {
    pub wfe_id: Uuid,
    pub status: WfeStatus,
    pub dynctx: Value,
    pub wfah: Vec<wfe_core::types::wfah::WfahEntry>,
    pub current_c_a: Value,
    pub end_response: Option<Value>,
}

impl WfeExecutor {
    /// Start a new WFE instance.
    pub async fn start(
        &self,
        wfd_id: Uuid,
        version: u32,
        actor: &Actor,
        input: &Value,
    ) -> Result<WfeStartResult, EngineError> {
        let wfd = self.wfd.fetch(wfd_id, version).await?;
        let orgtnt_id = self.org.orgtnt_for_orgu(actor.orgu_id).await?;

        let start_wfes = WFES {
            wfe_id: Uuid::new_v4(),
            dynctx: DynCtx::empty(),
            wfah: Wfah::empty(),
            status: WfeStatus::Active,
            orgtnt_id,
            wfd_id,
            wfd_version: version,
            current_c_a: vec![],
            end_response: None,
        };

        // Find a start rule the actor is eligible for.
        let start_rule = {
            let mut matched = None;
            for rule in &wfd.start {
                if wfe_core::engine::c_a_resolver::actor_in_c_a(
                    &rule.c_a,
                    actor,
                    &start_wfes,
                    &*self.org,
                )
                .await?
                {
                    matched = Some(rule);
                    break;
                }
            }
            matched
        };

        let start_rule = start_rule.ok_or(EngineError::StartNotEligible)?;

        let input_dynctx = match input {
            Value::Object(map) => DynCtx::empty().merge(map.clone()),
            _ => DynCtx::empty(),
        };
        let temp_wfe_id = Uuid::new_v4();
        let initial_dynctx = wfe_core::engine::dynctx_apply::apply(
            &input_dynctx,
            &start_rule.wfes_effects,
            actor,
            temp_wfe_id,
            "start",
            input,
        )?;

        let temp_wfes = WFES {
            wfe_id: temp_wfe_id,
            dynctx: initial_dynctx.clone(),
            wfah: Wfah::empty(),
            status: WfeStatus::Active,
            orgtnt_id,
            wfd_id,
            wfd_version: version,
            current_c_a: vec![],
            end_response: None,
        };
        let initial_c_a =
            resolve_wft_c_a(&start_rule.wft, &temp_wfes, actor.orgu_id, &*self.org).await?;

        let wfe_id = self
            .wfe
                .create_wfe(
                orgtnt_id,
                wfd_id,
                version,
                &initial_dynctx,
                &initial_c_a,
            )
            .await?;

        let entry = wfe_core::types::wfah::WfahEntry {
            seq: 1,
            action: start_rule.action.clone().unwrap_or_else(|| "start".into()),
            actor: actor.clone(),
            input: Some(input.clone()),
            applied_at: chrono::Utc::now(),
        };
        self.wfe.append_wfah(wfe_id, &entry).await?;

        let wfes = WFES {
            wfe_id,
            dynctx: initial_dynctx,
            wfah: Wfah(vec![entry]),
            status: WfeStatus::Active,
            orgtnt_id,
            wfd_id,
            wfd_version: version,
            current_c_a: initial_c_a,
            end_response: None,
        };
        let final_wfes = self.drain_autoexec(wfes, &wfd, actor).await?;

        Ok(WfeStartResult {
            wfe_id,
            current_c_a: final_wfes.current_c_a,
        })
    }

    /// Apply an action to an existing WFE.
    pub async fn apply(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
        action: &str,
        input: &Value,
    ) -> Result<WfeApplyResult, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;

        if wfes.status == WfeStatus::Terminal {
            return Err(EngineError::WfeTerminal);
        }

        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let (new_wfes, outcome) =
            apply_action(&wfes, actor, action, input, &wfd, &*self.org).await?;

        let dynctx_seq = new_wfes.wfah.entries().len() as u32;

        self.wfe
            .persist_new_dynctx(wfe_id, &new_wfes.dynctx, dynctx_seq)
            .await?;

        if let Some(entry) = new_wfes.wfah.entries().last() {
            self.wfe.append_wfah(wfe_id, entry).await?;
        }

        match outcome {
            WftOutcome::Terminal { end_response } => {
                self.wfe.set_terminal(wfe_id, &end_response).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal: true,
                    end_response: Some(end_response),
                    current_c_a: vec![],
                })
            }
            WftOutcome::NextCa(c_a) => {
                self.wfe.update_c_a(wfe_id, &c_a).await?;
                let mut next_wfes = new_wfes;
                next_wfes.current_c_a = c_a;
                let final_wfes = self.drain_autoexec(next_wfes, &wfd, actor).await?;
                Ok(WfeApplyResult {
                    wfe_id,
                    terminal: final_wfes.status == WfeStatus::Terminal,
                    end_response: final_wfes.end_response,
                    current_c_a: final_wfes.current_c_a,
                })
            }
        }
    }

    /// Query WFE state with visibility filtering applied.
    pub async fn query(&self, wfe_id: Uuid, viewer: &Actor) -> Result<WfeView, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;
        let filtered_ctx = visibility::apply(&wfes.dynctx, viewer, &wfd);

        Ok(WfeView {
            wfe_id,
            status: wfes.status.clone(),
            dynctx: filtered_ctx,
            wfah: wfes.wfah.entries().to_vec(),
            current_c_a: serde_json::to_value(&wfes.current_c_a).unwrap_or_default(),
            end_response: wfes.end_response.clone(),
        })
    }

    /// Returns action names the actor can perform on this WFE right now.
    pub async fn possible_actions(
        &self,
        wfe_id: Uuid,
        actor: &Actor,
    ) -> Result<Vec<String>, EngineError> {
        let wfes = self.wfe.load_wfes(wfe_id).await?;
        if wfes.status == WfeStatus::Terminal {
            return Ok(vec![]);
        }
        let wfd = self.wfd.fetch(wfes.wfd_id, wfes.wfd_version).await?;

        let mut actions = Vec::new();
        for t in &wfd.transitions {
            if zen::evaluate(&t.when, &wfes)? {
                if wfe_core::engine::c_a_resolver::actor_in_c_a(&t.c_a, actor, &wfes, &*self.org)
                    .await?
                {
                    if let Some(action_name) = &t.action {
                        actions.push(action_name.clone());
                    }
                }
            }
        }
        actions.dedup();
        Ok(actions)
    }

    async fn drain_autoexec(
        &self,
        mut wfes: WFES,
        wfd: &WFD,
        fallback_actor: &Actor,
    ) -> Result<WFES, EngineError> {
        let mut executed = std::collections::HashSet::new();

        loop {
            if wfes.status == WfeStatus::Terminal {
                return Ok(wfes);
            }

            let Some(transition) = wfd.transitions.iter().find(|t| {
                t.autoexec.is_some()
                    && !executed.contains(&t.id)
                    && zen::evaluate(&t.when, &wfes).unwrap_or(false)
            }) else {
                self.wfe.update_c_a(wfes.wfe_id, &wfes.current_c_a).await?;
                return Ok(wfes);
            };

            executed.insert(transition.id.clone());

            let system_actor = Actor {
                orgu_id: fallback_actor.orgu_id,
                user_id: fallback_actor.user_id,
                role: "system".into(),
            };
            let action = format!("auto:{}", transition.id);
            let input = json!({ "autoexec": transition.autoexec });

            let dynctx = dynctx_apply::apply(
                &wfes.dynctx,
                &transition.wfes_effects,
                &system_actor,
                wfes.wfe_id,
                &action,
                &input,
            )?;
            let wfah = wfes.wfah.push(action, system_actor, Some(input));
            let seq = wfah.entries().len() as u32;

            wfes.dynctx = dynctx;
            wfes.wfah = wfah;
            wfes.current_c_a = vec![];

            self.wfe
                .persist_new_dynctx(wfes.wfe_id, &wfes.dynctx, seq)
                .await?;
            if let Some(entry) = wfes.wfah.entries().last() {
                self.wfe.append_wfah(wfes.wfe_id, entry).await?;
            }

            match resolve_wft_outcome(&transition.wft, &wfes, fallback_actor.orgu_id, &*self.org)
                .await?
            {
                WftOutcome::Terminal { end_response } => {
                    self.wfe.set_terminal(wfes.wfe_id, &end_response).await?;
                    wfes.status = WfeStatus::Terminal;
                    wfes.end_response = Some(end_response);
                    wfes.current_c_a = vec![];
                    return Ok(wfes);
                }
                WftOutcome::NextCa(c_a) => {
                    wfes.current_c_a = c_a;
                    self.wfe.update_c_a(wfes.wfe_id, &wfes.current_c_a).await?;
                    if !wfes.current_c_a.is_empty() {
                        return Ok(wfes);
                    }
                }
            }
        }
    }
}

async fn resolve_wft_outcome(
    wft: &WftRule,
    wfes: &WFES,
    anchor: Uuid,
    org: &dyn OrgPort,
) -> Result<WftOutcome, EngineError> {
    match wft {
        WftRule::Simple { c_a } => {
            let c_a = resolve_c_a(c_a, anchor, wfes, org).await?;
            Ok(WftOutcome::NextCa(c_a))
        }
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if zen::evaluate(&cond.when, wfes)? {
                    if cond.terminal {
                        return Ok(WftOutcome::Terminal {
                            end_response: cond
                                .wfe_end_response
                                .clone()
                                .unwrap_or_else(|| json!({})),
                        });
                    }
                    if let Some(c_a) = &cond.c_a {
                        let c_a = resolve_c_a(c_a, anchor, wfes, org).await?;
                        return Ok(WftOutcome::NextCa(c_a));
                    }
                }
            }
            Ok(WftOutcome::NextCa(vec![]))
        }
        WftRule::Parallel { parallel, .. } => {
            let mut candidates = Vec::new();
            for branch in parallel {
                candidates.extend(resolve_c_a(&branch.c_a, anchor, wfes, org).await?);
            }
            candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
            Ok(WftOutcome::NextCa(candidates))
        }
    }
}

async fn resolve_wft_c_a(
    wft: &WftRule,
    wfes: &WFES,
    anchor: Uuid,
    org: &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    match wft {
        WftRule::Simple { c_a } => resolve_c_a(c_a, anchor, wfes, org).await,
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if !cond.terminal {
                    if let Some(c_a) = &cond.c_a {
                        return resolve_c_a(c_a, anchor, wfes, org).await;
                    }
                }
            }
            Ok(vec![])
        }
        WftRule::Parallel { parallel, .. } => {
            let mut candidates = Vec::new();
            for branch in parallel {
                candidates.extend(resolve_c_a(&branch.c_a, anchor, wfes, org).await?);
            }
            candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
            Ok(candidates)
        }
    }
}
