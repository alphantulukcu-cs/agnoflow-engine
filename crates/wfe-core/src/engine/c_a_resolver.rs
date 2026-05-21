use uuid::Uuid;
use crate::{
    error::EngineError,
    ports::OrgPort,
    types::actor::{Actor, CaRule, COrguExpr, CandidateActor},
};

/// Resolves a c_a rule array into a flat list of (orgu, role) candidate pairs.
/// Rules are OR'd — each rule independently contributes candidates.
pub async fn resolve_c_a(
    rules:          &[CaRule],
    anchor_orgu_id: Uuid,
    orgtnt_id:      Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<CandidateActor>, EngineError> {
    let mut candidates = Vec::new();
    for rule in rules {
        let orgus = resolve_c_orgu_for_rule(rule, anchor_orgu_id, orgtnt_id, org).await?;
        for unit in &orgus {
            for [_scope, role] in &rule.c_r {
                candidates.push(CandidateActor {
                    orgu_id: unit.orgu_id,
                    role:    role.clone(),
                });
            }
        }
    }
    candidates.dedup_by(|a, b| a.orgu_id == b.orgu_id && a.role == b.role);
    Ok(candidates)
}

/// Checks whether an actor satisfies at least one rule in the c_a array.
pub async fn actor_in_c_a(
    rules:     &[CaRule],
    actor:     &Actor,
    orgtnt_id: Uuid,
    org:       &dyn OrgPort,
) -> Result<bool, EngineError> {
    for rule in rules {
        if actor_matches_rule(rule, actor, orgtnt_id, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn actor_matches_rule(
    rule:      &CaRule,
    actor:     &Actor,
    orgtnt_id: Uuid,
    org:       &dyn OrgPort,
) -> Result<bool, EngineError> {
    let orgus = resolve_c_orgu_for_rule(rule, actor.orgu_id, orgtnt_id, org).await?;
    let actor_orgu_in_set = orgus.iter().any(|u| u.orgu_id == actor.orgu_id);
    if !actor_orgu_in_set {
        return Ok(false);
    }
    for [_scope, role] in &rule.c_r {
        if role == &actor.role {
            let has_role = org.check_user_role(actor.user_id, actor.orgu_id, role).await?;
            if has_role {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

async fn resolve_c_orgu_for_rule(
    rule:           &CaRule,
    anchor_orgu_id: Uuid,
    orgtnt_id:      Uuid,
    org:            &dyn OrgPort,
) -> Result<Vec<crate::types::actor::OrgUnit>, EngineError> {
    match &rule.c_orgu {
        COrguExpr::Expr(expr) => {
            org.resolve_c_orgu(anchor_orgu_id, expr, orgtnt_id).await
        }
        COrguExpr::Anchored { from: _, traverse } => {
            org.resolve_c_orgu(anchor_orgu_id, traverse, orgtnt_id).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use uuid::Uuid;
    use serde_json::json;
    use crate::{
        error::EngineError,
        ports::OrgPort,
        types::actor::{CaRule, COrguExpr, OrgUnit},
    };

    struct MockOrg { units: Vec<OrgUnit> }

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(&self, _anchor: Uuid, _expr: &str, _orgtnt_id: Uuid)
            -> Result<Vec<OrgUnit>, EngineError>
        {
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _u: Uuid, _o: Uuid, _r: &str)
            -> Result<bool, EngineError>
        {
            Ok(true)
        }
    }

    fn unit(id: &str) -> OrgUnit {
        OrgUnit {
            orgu_id:   Uuid::parse_str(id).unwrap(),
            orgu_type: json!({"type": "branch"}),
            path:      "1.10.100".into(),
        }
    }

    #[tokio::test]
    async fn resolves_single_rule() {
        let mock = MockOrg { units: vec![unit("00000000-0000-0000-0000-000000000001")] };
        let rule = CaRule {
            c_orgu: COrguExpr::Expr("self".into()),
            c_r:    vec![["self".into(), "clerk".into()]],
            c_u:    vec![],
        };

        let result = resolve_c_a(&[rule], Uuid::new_v4(), Uuid::new_v4(), &mock).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].role, "clerk");
    }

    #[tokio::test]
    async fn empty_rules_yields_no_candidates() {
        let mock = MockOrg { units: vec![] };
        let result = resolve_c_a(&[], Uuid::new_v4(), Uuid::new_v4(), &mock).await.unwrap();
        assert!(result.is_empty());
    }
}
