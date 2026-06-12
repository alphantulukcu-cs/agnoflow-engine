use crate::{
    engine::{c_a_resolver, dynctx_apply, permission},
    error::EngineError,
    ports::{OrgPort, WFES},
    types::{
        actor::{Actor, CandidateActor},
        wfd::{WftRule, WFD},
        wfe::WfeStatus,
    },
    zen,
};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug)]
pub enum WftOutcome {
    NextCa(Vec<CandidateActor>),
    Terminal { end_response: Value },
}

/// WFT(WFES, Actor, ACT) → (new WFES, WftOutcome)
/// Enforces permission, applies effects, appends WFAH, evaluates terminal_when and wft.
pub async fn apply_action(
    wfes: &WFES,
    actor: &Actor,
    action: &str,
    input: &Value,
    wfd: &WFD,
    org: &dyn OrgPort,
) -> Result<(WFES, WftOutcome), EngineError> {
    // 1. Permission check
    let permitted = permission::check(wfes, actor, action, &wfd.transitions, org).await?;
    if !permitted {
        return Err(EngineError::PermissionDenied(action.to_string()));
    }

    // 2. Find matching transition
    let transition = wfd
        .transitions
        .iter()
        .find(|t| {
            t.action.as_deref() == Some(action) && zen::evaluate(&t.when, wfes).unwrap_or(false)
        })
        .ok_or_else(|| EngineError::TransitionNotFound(action.to_string()))?;

    // 3. Apply wfes_effects → new DynCtx (immutable)
    let new_dynctx = dynctx_apply::apply(
        &wfes.dynctx,
        &transition.wfes_effects,
        actor,
        wfes.wfe_id,
        action,
        input,
    )?;

    // 4. Append to WFAH (immutable push)
    let new_wfah = wfes
        .wfah
        .push(action.to_string(), actor.clone(), Some(input.clone()));

    // 5. Build new WFES
    let new_wfes = WFES {
        wfe_id: wfes.wfe_id,
        dynctx: new_dynctx,
        wfah: new_wfah,
        status: WfeStatus::Active,
        orgtnt_id: wfes.orgtnt_id,
        wfd_id: wfes.wfd_id,
        wfd_version: wfes.wfd_version,
        current_c_a: vec![],
        end_response: None,
    };

    // 6. Check terminal_when. The editor may export rule references here
    // (e.g. "rules#terminal_1"); those are design-time ids, not ZEN.
    if terminal_when_matches(&wfd.terminal_when, &new_wfes)? {
        let end_response = build_end_response(&transition.wft, &new_wfes)?;
        return Ok((new_wfes, WftOutcome::Terminal { end_response }));
    }

    // 7. Resolve wft → new C_A or terminal branch
    let outcome = resolve_wft(&transition.wft, &new_wfes, actor.orgu_id, org).await?;
    Ok((new_wfes, outcome))
}

fn terminal_when_matches(expr: &str, wfes: &WFES) -> Result<bool, EngineError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() || trimmed == "false" || trimmed.starts_with("rules#") {
        return Ok(false);
    }
    zen::evaluate(trimmed, wfes)
}

async fn resolve_wft(
    wft: &WftRule,
    wfes: &WFES,
    anchor_orgu_id: Uuid,
    org: &dyn OrgPort,
) -> Result<WftOutcome, EngineError> {
    match wft {
        WftRule::Simple { c_a } => {
            let c_a = c_a_resolver::resolve_c_a(c_a, anchor_orgu_id, wfes, org).await?;
            Ok(WftOutcome::NextCa(c_a))
        }
        WftRule::Conditional { conditions } => {
            for cond in conditions {
                if zen::evaluate(&cond.when, wfes)? {
                    if cond.terminal {
                        let end_response = cond
                            .wfe_end_response
                            .as_ref()
                            .map(|resp| resolve_end_response_refs(resp, wfes))
                            .unwrap_or_else(|| json!({}));
                        return Ok(WftOutcome::Terminal { end_response });
                    }
                    if let Some(c_a) = &cond.c_a {
                        let c_a = c_a_resolver::resolve_c_a(c_a, anchor_orgu_id, wfes, org).await?;
                        return Ok(WftOutcome::NextCa(c_a));
                    }
                }
            }
            Ok(WftOutcome::NextCa(vec![]))
        }
        WftRule::Parallel { parallel, .. } => {
            let mut candidates = Vec::new();
            for branch in parallel {
                candidates.extend(
                    c_a_resolver::resolve_c_a(&branch.c_a, anchor_orgu_id, wfes, org).await?,
                );
            }
            candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
            Ok(WftOutcome::NextCa(candidates))
        }
    }
}

fn build_end_response(wft: &WftRule, wfes: &WFES) -> Result<Value, EngineError> {
    if let WftRule::Conditional { conditions } = wft {
        for cond in conditions {
            if cond.terminal && zen::evaluate(&cond.when, wfes).unwrap_or(false) {
                if let Some(resp) = &cond.wfe_end_response {
                    return Ok(resolve_end_response_refs(resp, wfes));
                }
            }
        }
    }
    Ok(json!({}))
}

fn resolve_end_response_refs(template: &Value, wfes: &WFES) -> Value {
    match template {
        Value::Object(map) => {
            if let Some(path) = map.get("ref").and_then(|v| v.as_str()) {
                if let Some(stripped) = path.strip_prefix("$ctx.") {
                    let mut current = wfes.dynctx.as_value();
                    for part in stripped.split('.') {
                        current = match current.get(part) {
                            Some(v) => v,
                            None => return Value::Null,
                        };
                    }
                    return current.clone();
                }
            }
            Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), resolve_end_response_refs(v, wfes)))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::EngineError,
        ports::{OrgPort, WFES},
        types::{
            actor::{Actor, COrguExpr, CaRule, OrgUnit},
            dynctx::DynCtx,
            wfah::Wfah,
            wfd::{ActionDef, ActionInput, EffectValue, Transition, WfesEffects, WftRule, WFD},
            wfe::WfeStatus,
        },
    };
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::HashMap;
    use uuid::Uuid;

    struct AlwaysMatchOrg {
        orgu_id: Uuid,
    }
    #[async_trait]
    impl OrgPort for AlwaysMatchOrg {
        async fn resolve_c_orgu(
            &self,
            _a: Uuid,
            _e: &str,
            _t: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![OrgUnit {
                orgu_id: self.orgu_id,
                orgu_type: json!({}),
                path: "1".into(),
            }])
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _orgu_id: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::new_v4())
        }
    }

    fn make_wfd(terminal_when: &str) -> WFD {
        let mut actions = HashMap::new();
        actions.insert(
            "approve".into(),
            ActionDef {
                name: "approve".into(),
                description: None,
                input: ActionInput::default(),
            },
        );
        let mut effects = WfesEffects::default();
        effects
            .set
            .insert("status".into(), EffectValue::Literal(json!("approved")));
        WFD {
            id: Uuid::new_v4().to_string(),
            name: "test".into(),
            version: "1.0.0".into(),
            description: None,
            context: json!({}),
            start: vec![],
            actions,
            transitions: vec![Transition {
                id: "t1".into(),
                when: "$status == 'pending'".into(),
                action: Some("approve".into()),
                autoexec: None,
                c_a: vec![CaRule {
                    c_orgu: COrguExpr::Expr("self".into()),
                    c_r: vec!["clerk".into()],
                    c_u: vec![],
                }],
                wfes_effects: effects,
                trigger: None,
                wft: WftRule::Simple { c_a: vec![] },
            }],
            listable: vec![],
            terminal_when: terminal_when.into(),
            extra: Default::default(),
        }
    }

    fn actor(orgu_id: Uuid) -> Actor {
        Actor {
            orgu_id,
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        }
    }

    fn wfes(_orgu_id: Uuid) -> WFES {
        let dynctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!("pending"));
            m
        });
        WFES {
            wfe_id: Uuid::new_v4(),
            dynctx,
            wfah: Wfah::empty(),
            status: WfeStatus::Active,
            orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(),
            wfd_version: 1,
            current_c_a: vec![],
            end_response: None,
        }
    }

    #[tokio::test]
    async fn apply_action_updates_dynctx() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let (new_wfes, _outcome) =
            apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
                .await
                .unwrap();
        assert_eq!(new_wfes.dynctx.get("status"), Some(&json!("approved")));
    }

    #[tokio::test]
    async fn apply_action_appends_wfah() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let (new_wfes, _outcome) =
            apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
                .await
                .unwrap();
        assert_eq!(new_wfes.wfah.entries().len(), 1);
        assert_eq!(new_wfes.wfah.entries()[0].action, "approve");
    }

    #[tokio::test]
    async fn apply_action_returns_terminal_when_condition_met() {
        let orgu_id = Uuid::new_v4();
        let org = AlwaysMatchOrg { orgu_id };
        let wfd = make_wfd("$status == 'approved'");
        let w = wfes(orgu_id);

        let (_new_wfes, outcome) =
            apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
                .await
                .unwrap();
        assert!(matches!(outcome, WftOutcome::Terminal { .. }));
    }

    #[tokio::test]
    async fn permission_denied_returns_error() {
        struct NoMatchOrg;
        #[async_trait]
        impl OrgPort for NoMatchOrg {
            async fn resolve_c_orgu(
                &self,
                _a: Uuid,
                _e: &str,
                _t: Uuid,
            ) -> Result<Vec<OrgUnit>, EngineError> {
                Ok(vec![])
            }
            async fn check_user_role(
                &self,
                _u: Uuid,
                _o: Uuid,
                _r: &str,
            ) -> Result<bool, EngineError> {
                Ok(false)
            }
            async fn orgtnt_for_orgu(&self, _orgu_id: Uuid) -> Result<Uuid, EngineError> {
                Ok(Uuid::new_v4())
            }
        }
        let orgu_id = Uuid::new_v4();
        let org = NoMatchOrg;
        let wfd = make_wfd("$status == 'never'");
        let w = wfes(orgu_id);

        let err = apply_action(&w, &actor(orgu_id), "approve", &json!({}), &wfd, &org)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::PermissionDenied(_)));
    }
}
