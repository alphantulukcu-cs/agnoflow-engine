use crate::{
    engine::c_a_resolver::actor_in_c_a,
    error::EngineError,
    ports::{OrgPort, WFES},
    types::{actor::Actor, wfd::Transition},
    zen,
};

/// P(WFES, Actor, ACT) → bool
/// Finds transitions matching the action + when condition, then checks actor in c_a.
pub async fn check(
    wfes: &WFES,
    actor: &Actor,
    action: &str,
    transitions: &[Transition],
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    for t in transitions {
        if t.action.as_deref() != Some(action) {
            continue;
        }
        let when_matches = zen::evaluate(&t.when, wfes)?;
        if !when_matches {
            continue;
        }
        if actor_in_c_a(&t.c_a, actor, wfes, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
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
            wfd::{Transition, WfesEffects, WftRule},
            wfe::WfeStatus,
        },
    };
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

    struct AlwaysMatchOrg;
    #[async_trait]
    impl OrgPort for AlwaysMatchOrg {
        async fn resolve_c_orgu(
            &self,
            anchor: Uuid,
            _e: &str,
            _t: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![OrgUnit {
                orgu_id: anchor,
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

    struct NeverMatchOrg;
    #[async_trait]
    impl OrgPort for NeverMatchOrg {
        async fn resolve_c_orgu(
            &self,
            _a: Uuid,
            _e: &str,
            _t: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(vec![])
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str) -> Result<bool, EngineError> {
            Ok(false)
        }
        async fn orgtnt_for_orgu(&self, _orgu_id: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::new_v4())
        }
    }

    fn wfes(status: &str) -> WFES {
        let dynctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!(status));
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

    fn transition(action: &str, when: &str) -> Transition {
        Transition {
            id: "t1".into(),
            when: when.into(),
            action: Some(action.into()),
            autoexec: None,
            c_a: vec![CaRule {
                c_orgu: COrguExpr::Expr("self".into()),
                c_r: vec!["clerk".into()],
                c_u: vec![],
            }],
            wfes_effects: WfesEffects::default(),
            trigger: None,
            wft: WftRule::Simple { c_a: vec![] },
        }
    }

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        }
    }

    #[tokio::test]
    async fn permitted_when_actor_matches() {
        let t = transition("approve", "$status == 'pending'");
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &AlwaysMatchOrg)
            .await
            .unwrap();
        assert!(result);
    }

    #[tokio::test]
    async fn denied_when_actor_not_in_c_a() {
        let t = transition("approve", "$status == 'pending'");
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &NeverMatchOrg)
            .await
            .unwrap();
        assert!(!result);
    }

    #[tokio::test]
    async fn denied_when_when_condition_false() {
        let t = transition("approve", "$status == 'approved'");
        let wfes = wfes("pending");
        let result = check(&wfes, &actor(), "approve", &[t], &AlwaysMatchOrg)
            .await
            .unwrap();
        assert!(!result);
    }
}
