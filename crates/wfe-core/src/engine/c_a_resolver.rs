use crate::{
    error::EngineError,
    ports::{OrgPort, WFES},
    types::actor::{Actor, COrguExpr, COrguFrom, CaRule, CandidateActor},
};
use uuid::Uuid;

/// Resolves a c_a rule array into a flat list of (orgu, role) candidate pairs.
/// Rules are OR'd — each rule independently contributes candidates.
pub async fn resolve_c_a(
    rules: &[CaRule],
    anchor_orgu_id: Uuid,
    wfes: &WFES,
    org: &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    let mut candidates = Vec::new();
    for rule in rules {
        let orgus = resolve_c_orgu_for_rule(rule, anchor_orgu_id, wfes, org).await?;
        for unit in &orgus {
            for role in &rule.c_r {
                candidates.push(CandidateActor {
                    orgu_id: unit.orgu_id,
                    role: role.clone(),
                });
            }
        }
    }
    candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
    Ok(candidates)
}

/// Checks whether an actor satisfies at least one rule in the c_a array.
pub async fn actor_in_c_a(
    rules: &[CaRule],
    actor: &Actor,
    wfes: &WFES,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    for rule in rules {
        if actor_matches_rule(rule, actor, wfes, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn actor_matches_rule(
    rule: &CaRule,
    actor: &Actor,
    wfes: &WFES,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    let orgus = resolve_c_orgu_for_rule(rule, actor.orgu_id, wfes, org).await?;
    let actor_orgu_in_set = orgus.iter().any(|u| u.orgu_id == actor.orgu_id);
    if !actor_orgu_in_set {
        return Ok(false);
    }
    for role in &rule.c_r {
        if role == &actor.role {
            let has_role = org
                .check_user_role(actor.user_id, actor.orgu_id, role)
                .await?;
            if has_role {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn resolve_c_orgu_for_rule(
    rule: &CaRule,
    anchor_orgu_id: Uuid,
    wfes: &WFES,
    org: &dyn OrgPort,
) -> Result<Vec<crate::types::actor::OrgUnit>, EngineError> {
    match &rule.c_orgu {
        COrguExpr::Expr(expr) => {
            org.resolve_c_orgu(anchor_orgu_id, expr, wfes.orgtnt_id)
                .await
        }
        COrguExpr::Anchored { from, traverse } => {
            let anchor = match from {
                COrguFrom::DynCtx(path) => resolve_anchor_from_dynctx(path, wfes)?.unwrap_or(anchor_orgu_id),
                COrguFrom::Wfah(query) => {
                    wfes.wfah.entries().iter().rev()
                        .find(|e| e.action == query.wfah)
                        .map(|e| e.actor.orgu_id)
                        .unwrap_or(anchor_orgu_id)
                }
            };
            let expr = normalize_traverse(traverse);
            org.resolve_c_orgu(anchor, &expr, wfes.orgtnt_id).await
        }
    }
}

fn normalize_traverse(traverse: &str) -> String {
    if traverse == "self" || traverse.starts_with("self.") {
        traverse.to_string()
    } else {
        format!("self.{traverse}")
    }
}

fn resolve_anchor_from_dynctx(from: &str, wfes: &WFES) -> Result<Option<Uuid>, EngineError> {
    let stripped = from.strip_prefix("$ctx.").unwrap_or(from);
    let mut current = wfes.dynctx.as_value();
    for part in stripped.split('.') {
        current = match current.get(part) {
            Some(value) => value,
            None => return Ok(None),
        };
    }

    let raw = if let Some(s) = current.as_str() {
        Some(s)
    } else if let Some(obj) = current.as_object() {
        obj.get("orgu")
            .or_else(|| obj.get("orgu_id"))
            .and_then(|v| v.as_str())
    } else {
        None
    };

    raw.map(|s| {
        Uuid::parse_str(s).map_err(|e| {
            EngineError::EffectValue(format!("invalid c_orgu anchor UUID at {from}: {e}"))
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        error::EngineError,
        ports::OrgPort,
        types::actor::{COrguExpr, CaRule, OrgUnit},
    };
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

    struct MockOrg {
        units: Vec<OrgUnit>,
    }

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(
            &self,
            _anchor: Uuid,
            _expr: &str,
            _orgtnt_id: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str) -> Result<bool, EngineError> {
            Ok(true)
        }
        async fn orgtnt_for_orgu(&self, _orgu_id: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::new_v4())
        }
    }

    fn unit(id: &str) -> OrgUnit {
        OrgUnit {
            orgu_id: Uuid::parse_str(id).unwrap(),
            orgu_type: json!({"type": "branch"}),
            path: "1.10.100".into(),
        }
    }

    #[tokio::test]
    async fn resolves_single_rule() {
        let mock = MockOrg {
            units: vec![unit("00000000-0000-0000-0000-000000000001")],
        };
        let rule = CaRule {
            c_orgu: COrguExpr::Expr("self".into()),
            c_r: vec!["clerk".into()],
            c_u: vec![],
        };

        let wfes = WFES {
            wfe_id: Uuid::new_v4(),
            dynctx: crate::types::dynctx::DynCtx::empty(),
            wfah: crate::types::wfah::Wfah::empty(),
            status: crate::types::wfe::WfeStatus::Active,
            orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(),
            wfd_version: 1,
            current_c_a: vec![],
            end_response: None,
        };
        let result = resolve_c_a(&[rule], Uuid::new_v4(), &wfes, &mock)
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "clerk");
    }

    #[tokio::test]
    async fn empty_rules_yields_no_candidates() {
        let mock = MockOrg { units: vec![] };
        let wfes = WFES {
            wfe_id: Uuid::new_v4(),
            dynctx: crate::types::dynctx::DynCtx::empty(),
            wfah: crate::types::wfah::Wfah::empty(),
            status: crate::types::wfe::WfeStatus::Active,
            orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(),
            wfd_version: 1,
            current_c_a: vec![],
            end_response: None,
        };
        let result = resolve_c_a(&[], Uuid::new_v4(), &wfes, &mock)
            .await
            .unwrap();
        assert!(result.is_empty());
    }
}
