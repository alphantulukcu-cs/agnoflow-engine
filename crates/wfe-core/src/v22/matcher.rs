//! §3 C_A Authorization Matcher (WOR-37) — kanonik semantik:
//!
//! ```text
//! match(rule, actor, wfe) :=
//!   actor.orgu ∈ resolve(rule.c_orgu, wfe)
//!   AND ( (rule.c_r var ve actor.role ∈ rule.c_r ve rol ataması doğrulanır)
//!         OR (rule.c_u var ve actor.user ∈ rule.c_u) )   # rol-agnostik
//! ```
//!
//! Verilmeyen alan false'dur (wildcard değil). Bu matcher node c_a, start `from`
//! node'unun c_a'sı, transition ek-kısıt c_a ve listable[].c_a için AYNIDIR.

use crate::error::EngineError;
use crate::ports::OrgPort;
use crate::types::actor::Actor;
use crate::types::wfah::Wfah;
use crate::types::wfd_v22::CandidateActor;
use crate::v22::resolver::resolve_c_orgu;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Matcher'ın ihtiyaç duyduğu WFE bağlamı.
#[derive(Clone, Copy)]
pub struct MatchEnv<'a> {
    pub ctx: &'a Value,
    pub wfah: &'a Wfah,
    pub orgtnt_id: Uuid,
}

pub async fn authorize(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    // 1. ORGU kanalı: actor.orgu resolve edilen kümede olmalı
    let resolved = resolve_c_orgu(
        &rule.c_orgu,
        actor.orgu_id,
        env.ctx,
        env.wfah,
        env.orgtnt_id,
        org,
    )
    .await?;
    if !resolved.iter().any(|u| u.orgu_id == actor.orgu_id) {
        return Ok(false);
    }

    // 2. Rol kanalı: c_r listesinde VE gerçekten atanmış
    if let Some(c_r) = &rule.c_r {
        if c_r.iter().any(|r| r == &actor.role)
            && org
                .check_user_role(actor.user_id, actor.orgu_id, &actor.role)
                .await?
        {
            return Ok(true);
        }
    }

    // 3. Kullanıcı kanalı: rol-agnostik — username veya UUID string eşleşmesi
    if let Some(c_u) = &rule.c_u {
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

    Ok(false)
}

/// Madde 6: yetki kararı — doğrudan mı, vekaleten mi (audit için provenance taşır).
#[derive(Debug, Clone, PartialEq)]
pub enum AuthDecision {
    Denied,
    /// Aktör kurala DOĞRUDAN uyuyor (bugünkü davranış).
    Direct,
    /// Aktör kurala VEKALETEN uyuyor: bir vekâlet vereni (delegator) temsil ediyor.
    Delegated {
        delegation_id: Uuid,
        delegator_user_id: Uuid,
        seat_orgu_id: Uuid,
        seat_role: String,
    },
}

impl AuthDecision {
    pub fn is_authorized(&self) -> bool {
        !matches!(self, AuthDecision::Denied)
    }
}

/// Madde 6: vekalet-farkında yetkilendirme. Önce doğrudan `authorize`; olmazsa
/// claimant'a o an geçerli her vekâlet için (a) claimant `grantee`'ye uyuyor mu VE
/// (b) delegator'ın koltuğunu temsil eden SENTETİK aktör kurala uyuyor mu.
///
/// İki iç çağrı da düz `authorize`'dır (vekalet-farkında DEĞİL) → tek seviye; zincir
/// (transitif vekalet) oluşmaz. `grantee` claimant'ın kendi anchor'ıyla değerlendirilir
/// (kişi = c_u eşleşmesi; havuz = claimant'ın rol/orgu'su).
pub async fn authorize_with_delegation(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
    now: DateTime<Utc>,
) -> Result<AuthDecision, EngineError> {
    if authorize(rule, actor, env, org).await? {
        return Ok(AuthDecision::Direct);
    }
    let grants = org
        .active_delegations_for(actor.user_id, env.orgtnt_id, now)
        .await?;
    for g in grants {
        // (a) claimant gerçekten bu vekâletin alıcısı mı? (kişi veya havuz)
        if !authorize(&g.grantee, actor, env, org).await? {
            continue;
        }
        // (b) delegator'ın koltuğu bu kurala uyuyor mu? (sentetik aktör)
        let synthetic = Actor {
            orgu_id: g.seat_orgu_id,
            user_id: g.delegator_user_id,
            role: g.seat_role.clone(),
        };
        if authorize(rule, &synthetic, env, org).await? {
            return Ok(AuthDecision::Delegated {
                delegation_id: g.delegation_id,
                delegator_user_id: g.delegator_user_id,
                seat_orgu_id: g.seat_orgu_id,
                seat_role: g.seat_role,
            });
        }
    }
    Ok(AuthDecision::Denied)
}

/// `authorize_with_delegation`'ın bool kısayolu — provenance gerekmeyen çağıranlar
/// (visibility, listable, field x-visibility) için. `now = Utc::now()`.
pub async fn authorize_or_delegated(
    rule: &CandidateActor,
    actor: &Actor,
    env: MatchEnv<'_>,
    org: &dyn OrgPort,
) -> Result<bool, EngineError> {
    Ok(authorize_with_delegation(rule, actor, env, org, Utc::now())
        .await?
        .is_authorized())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::OrgUnit;
    use crate::types::wfd_v22::COrgu;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockOrg {
        units: Vec<OrgUnit>,
        role_assigned: bool,
        ident: Option<String>,
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
        async fn user_ident(&self, _: Uuid) -> Result<Option<String>, EngineError> {
            Ok(self.ident.clone())
        }
    }

    fn unit(id: Uuid) -> OrgUnit {
        OrgUnit {
            orgu_id: id,
            orgu_type: json!({}),
            path: "1".into(),
        }
    }

    fn rule(c_r: Option<Vec<&str>>, c_u: Option<Vec<&str>>) -> CandidateActor {
        CandidateActor {
            c_orgu: COrgu::Selector("self".into()),
            c_r: c_r.map(|v| v.into_iter().map(String::from).collect()),
            c_u: c_u.map(|v| v.into_iter().map(String::from).collect()),
        }
    }

    fn actor(orgu: Uuid, role: &str) -> Actor {
        Actor {
            orgu_id: orgu,
            user_id: Uuid::new_v4(),
            role: role.into(),
        }
    }

    fn env<'a>(ctx: &'a Value, wfah: &'a Wfah) -> MatchEnv<'a> {
        MatchEnv {
            ctx,
            wfah,
            orgtnt_id: Uuid::nil(),
        }
    }

    static EMPTY_CTX: std::sync::LazyLock<Value> = std::sync::LazyLock::new(|| json!({}));
    static EMPTY_WFAH: std::sync::LazyLock<Wfah> = std::sync::LazyLock::new(Wfah::empty);

    #[tokio::test]
    async fn role_match_with_assignment_succeeds() {
        let orgu = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: true, ident: None };
        let a = actor(orgu, "creditAnalyst");
        assert!(authorize(&rule(Some(vec!["creditAnalyst"]), None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn role_match_without_assignment_fails() {
        let orgu = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: false, ident: None };
        let a = actor(orgu, "creditAnalyst");
        assert!(!authorize(&rule(Some(vec!["creditAnalyst"]), None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn actor_outside_resolved_orgu_fails_even_with_role() {
        let orgu = Uuid::new_v4();
        let other = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(other)], role_assigned: true, ident: None };
        let a = actor(orgu, "creditAnalyst");
        assert!(!authorize(&rule(Some(vec!["creditAnalyst"]), None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn c_u_match_is_role_agnostic() {
        let orgu = Uuid::new_v4();
        let org = MockOrg {
            units: vec![unit(orgu)],
            role_assigned: false, // rol ataması YOK — yine de c_u geçmeli
            ident: Some("user_ayse".into()),
        };
        let a = actor(orgu, "branchClerk");
        assert!(authorize(&rule(None, Some(vec!["user_ayse"])), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn c_u_match_by_uuid_string() {
        let orgu = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: false, ident: None };
        let a = actor(orgu, "x");
        let uuid_str = a.user_id.to_string();
        let r = CandidateActor {
            c_orgu: COrgu::Selector("self".into()),
            c_r: None,
            c_u: Some(vec![uuid_str]),
        };
        assert!(authorize(&r, &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org).await.unwrap());
    }

    #[tokio::test]
    async fn missing_both_channels_is_false_not_wildcard() {
        let orgu = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: true, ident: Some("u".into()) };
        let a = actor(orgu, "any");
        assert!(!authorize(&rule(None, None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn role_in_list_but_different_actor_role_fails() {
        let orgu = Uuid::new_v4();
        let org = MockOrg { units: vec![unit(orgu)], role_assigned: true, ident: None };
        let a = actor(orgu, "branchClerk");
        assert!(!authorize(&rule(Some(vec!["creditAnalyst"]), None), &a, env(&EMPTY_CTX, &EMPTY_WFAH), &org)
            .await
            .unwrap());
    }
}
