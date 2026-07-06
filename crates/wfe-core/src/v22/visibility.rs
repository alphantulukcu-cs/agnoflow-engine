//! §4 Visibility Matcher (WOR-38, M13) — authorization'dan AYRI fonksiyon, kriterler arası OR:
//!
//! ```text
//! visible(vis, actor, wfe) :=
//!      (vis.c_orgu var ve actor.orgu ∈ resolve(vis.c_orgu, wfe))
//!   OR (vis.c_r    var ve actor.role ∈ vis.c_r)          # scope'suz
//!   OR (vis.c_u    var ve actor.user ∈ vis.c_u)          # scope'suz
//!   OR (vis.c_a    var ve match(vis.c_a, actor, wfe))    # scope'lu tam kural
//! ```
//!
//! V yalnızca field okunurluğunu filtreler; ACT/claim/listability üretmez.

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfd_v22::{CandidateActor, COrgu};
use crate::v22::matcher::{authorize, MatchEnv};
use crate::v22::resolver::resolve_c_orgu;
use serde::Deserialize;
use serde_json::Value;

/// `x-visibility` bloğu — context şemasındaki bir property üzerinde.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct XVisibility {
    #[serde(default)]
    pub c_orgu: Option<COrgu>,
    #[serde(default)]
    pub c_r: Option<Vec<String>>,
    #[serde(default)]
    pub c_u: Option<Vec<String>>,
    #[serde(default)]
    pub c_a: Option<CandidateActor>,
}

pub async fn visible(
    vis: &XVisibility,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    if let Some(c_orgu) = &vis.c_orgu {
        let resolved = resolve_c_orgu(c_orgu, actor.orgu_id, env.ctx, env.wfah, env.orgtnt_id, org)
            .await?;
        if resolved.iter().any(|u| u.orgu_id == actor.orgu_id) {
            return Ok(true);
        }
    }
    if let Some(c_r) = &vis.c_r {
        if c_r.iter().any(|r| r == &actor.role) {
            return Ok(true);
        }
    }
    if let Some(c_u) = &vis.c_u {
        let uuid_str = actor.user_id.to_string();
        if c_u.iter().any(|u| u == &uuid_str) {
            return Ok(true);
        }
        if let Some(ident) = org.user_ident(actor.user_id).await? {
            if c_u.iter().any(|u| u == &ident) {
                return Ok(true);
            }
        }
    }
    if let Some(c_a) = &vis.c_a {
        if authorize(c_a, actor, env, org).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// DynCtx'i context şemasındaki `x-visibility` kurallarına göre filtreler.
/// `x-visibility` olmayan field herkese görünür. Match etmeyen field response'tan çıkarılır.
/// Nested obje şemaları recursive işlenir.
pub async fn filter_dynctx(
    context_schema: &Value,
    dynctx: &Value,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<Value, EngineError> {
    let Value::Object(data) = dynctx else {
        return Ok(dynctx.clone());
    };
    let props = context_schema.get("properties").and_then(Value::as_object);

    let mut out = serde_json::Map::new();
    for (key, value) in data {
        let field_schema = props.and_then(|p| p.get(key));
        if let Some(schema) = field_schema {
            if let Some(vis_value) = schema.get("x-visibility") {
                let vis: XVisibility = serde_json::from_value(vis_value.clone())
                    .map_err(|e| EngineError::InvalidWfd(format!("x-visibility parse: {e}")))?;
                if !visible(&vis, actor, env, org).await? {
                    continue;
                }
            }
            // nested obje şeması varsa recursive filtrele
            if schema.get("properties").is_some() && value.is_object() {
                let filtered = Box::pin(filter_dynctx(schema, value, actor, env, org)).await?;
                out.insert(key.clone(), filtered);
                continue;
            }
        }
        out.insert(key.clone(), value.clone());
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfah::Wfah;
    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

    struct MockOrg {
        units: Vec<OrgUnit>,
        role_assigned: bool,
    }

    #[async_trait]
    impl OrgPort for MockOrg {
        async fn resolve_c_orgu(
            &self,
            _: Uuid,
            _: &str,
            _: Uuid,
        ) -> Result<Vec<OrgUnit>, EngineError> {
            Ok(self.units.clone())
        }
        async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
            Ok(self.role_assigned)
        }
        async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
            Ok(Uuid::nil())
        }
    }

    fn actor(orgu: Uuid, role: &str) -> Actor {
        Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: role.into(),
        }
    }

    static EMPTY_CTX: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));
    static EMPTY_WFAH: std::sync::LazyLock<Wfah> = std::sync::LazyLock::new(Wfah::empty);

    fn env<'a>() -> MatchEnv<'a> {
        MatchEnv {
            ctx: &EMPTY_CTX,
            wfah: &EMPTY_WFAH,
            orgtnt_id: Uuid::nil(),
        }
    }

    #[tokio::test]
    async fn c_r_criterion_is_scopeless_or() {
        // rol listede ama ORGU resolve edilen kümede DEĞİL — yine de görünür (scope'suz)
        let org = MockOrg { units: vec![], role_assigned: false };
        let vis = XVisibility {
            c_r: Some(vec!["creditDeptManager".into(), "branchManager".into()]),
            ..Default::default()
        };
        let a = actor(Uuid::new_v4(), "branchManager");
        assert!(visible(&vis, &a, env(), &org).await.unwrap());

        let b = actor(Uuid::new_v4(), "clerk");
        assert!(!visible(&vis, &b, env(), &org).await.unwrap());
    }

    #[tokio::test]
    async fn c_a_criterion_uses_full_authorization_rule() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![OrgUnit { orgu_id: orgu, orgu_type: json!({}), path: "1".into() }],
            role_assigned: true,
        };
        let vis = XVisibility {
            c_a: Some(CandidateActor {
                c_orgu: COrgu::Selector("self".into()),
                c_r: Some(vec!["auditor".into()]),
                c_u: None,
            }),
            ..Default::default()
        };
        let a = actor(orgu, "auditor");
        assert!(visible(&vis, &a, env(), &org).await.unwrap());
        let b = actor(orgu, "clerk");
        assert!(!visible(&vis, &b, env(), &org).await.unwrap());
    }

    #[tokio::test]
    async fn filter_hides_restricted_field_from_non_matching_actor() {
        let org = MockOrg { units: vec![], role_assigned: false };
        let schema = json!({
            "type": "object",
            "properties": {
                "amount": {"type": "number"},
                "internal_notes": {
                    "type": "string",
                    "x-visibility": {"c_r": ["creditDeptManager", "branchManager"]}
                }
            }
        });
        let ctx = json!({"amount": 5000, "internal_notes": "gizli"});

        let manager = actor(Uuid::new_v4(), "branchManager");
        let filtered = filter_dynctx(&schema, &ctx, &manager, env(), &org).await.unwrap();
        assert_eq!(filtered["internal_notes"], json!("gizli"));

        let clerk = actor(Uuid::new_v4(), "branchClerk");
        let filtered = filter_dynctx(&schema, &ctx, &clerk, env(), &org).await.unwrap();
        assert!(filtered.get("internal_notes").is_none(), "match etmeyen actor'a gizlenmeli");
        assert_eq!(filtered["amount"], json!(5000), "kısıtsız field görünmeli");
    }

    #[tokio::test]
    async fn fields_without_visibility_are_visible_to_all() {
        let org = MockOrg { units: vec![], role_assigned: false };
        let schema = json!({"type": "object", "properties": {"x": {"type": "string"}}});
        let ctx = json!({"x": "açık", "undeclared": 1});
        let a = actor(Uuid::new_v4(), "anyone");
        let filtered = filter_dynctx(&schema, &ctx, &a, env(), &org).await.unwrap();
        assert_eq!(filtered["x"], json!("açık"));
        assert_eq!(filtered["undeclared"], json!(1));
    }
}
