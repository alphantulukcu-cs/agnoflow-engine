//! Madde 6: vekalet/delegasyon — engine (matcher + pipeline) testleri.
//!
//! Senaryo: node `self__creditAnalyst` (c_a = {c_orgu:"self", c_r:["creditAnalyst"]}).
//! Ahmet bu koltuğu (creditAnalyst @ orguA) taşır ve izne çıkarken vekalet verir.
//! Vekil, doğrudan creditAnalyst OLMADAN, vekaleten claim uygunluğu kazanmalı.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::delegation::DelegationGrant;
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::{AutoexecDef, COrgu, CandidateActor, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::matcher::AuthDecision;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure, Wfes};

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).unwrap()
}

// ---- mock org: rol atamaları + username haritası + vekalet listesi yapılandırılabilir ----

struct MockOrg {
    /// (user_id, orgu_id, role) — "gerçekten atanmış" koltuklar (check_user_role).
    roles: Vec<(Uuid, Uuid, String)>,
    /// user_id → username (c_u eşleşmesi için).
    idents: Vec<(Uuid, String)>,
    /// active_delegations_for'un döndüreceği adaylar (grantee filtresi matcher'da).
    delegations: Vec<DelegationGrant>,
}

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        // "self" vb. → anchor'ın kendi orgu'su.
        Ok(vec![OrgUnit {
            orgu_id: anchor,
            orgu_type: json!({"type": "branch"}),
            path: "1".into(),
        }])
    }
    async fn check_user_role(
        &self,
        user_id: Uuid,
        orgu_id: Uuid,
        role_name: &str,
    ) -> Result<bool, EngineError> {
        Ok(self
            .roles
            .iter()
            .any(|(u, o, r)| *u == user_id && *o == orgu_id && r == role_name))
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
    async fn user_ident(&self, user_id: Uuid) -> Result<Option<String>, EngineError> {
        Ok(self
            .idents
            .iter()
            .find(|(u, _)| *u == user_id)
            .map(|(_, name)| name.clone()))
    }
    async fn active_delegations_for(
        &self,
        _claimant_user_id: Uuid,
        _orgtnt_id: Uuid,
        _now: DateTime<Utc>,
    ) -> Result<Vec<DelegationGrant>, EngineError> {
        Ok(self.delegations.clone())
    }
}

struct NoRunner;
#[async_trait]
impl AutoexecRunner for NoRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        Err(ExecFailure::failed("çağrılmamalı"))
    }
}

fn actor(orgu: Uuid, user: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: user,
        role: role.into(),
    }
}

fn wfes_at(node: &str) -> Wfes {
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    let wfah = Wfah::empty().push("start".into(), system, None);
    let created_at = wfah.entries()[0].applied_at;
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(json!({})),
        wfah,
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
    }
}

/// Ahmet (delegator) creditAnalyst @ orgu taşır; grantee kuralı ile vekalet üretir.
fn grant(delegator: Uuid, seat_orgu: Uuid, grantee: CandidateActor) -> DelegationGrant {
    DelegationGrant {
        delegation_id: Uuid::new_v4(),
        delegator_user_id: delegator,
        seat_orgu_id: seat_orgu,
        seat_role: "creditAnalyst".into(),
        grantee,
    }
}

fn person_grantee(username: &str) -> CandidateActor {
    CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: None,
        c_u: Some(vec![username.into()]),
    }
}

fn pool_grantee(role: &str) -> CandidateActor {
    CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: Some(vec![role.into()]),
        c_u: None,
    }
}

// ================================================================ testler

#[tokio::test]
async fn delegated_person_can_claim() {
    let orgu = Uuid::new_v4();
    let ahmet = Uuid::new_v4();
    let ayse = Uuid::new_v4();
    let org = MockOrg {
        roles: vec![(ahmet, orgu, "creditAnalyst".into())], // Ahmet gerçekten analist
        idents: vec![(ayse, "ayse".into())],
        delegations: vec![grant(ahmet, orgu, person_grantee("ayse"))],
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    // Ayşe clerk — DOĞRUDAN uygun değil, ama vekaleten uygun olmalı.
    let ayse_actor = actor(orgu, ayse, "clerk");
    assert_eq!(
        engine
            .can_claim(&wfd, &wfes, &ayse_actor, None)
            .await
            .unwrap(),
        ClaimCheck::Ok
    );

    // provenance: Delegated(Ahmet).
    match engine
        .claim_decision(&wfd, &wfes, &ayse_actor, None)
        .await
        .unwrap()
    {
        AuthDecision::Delegated {
            delegator_user_id,
            seat_role,
            ..
        } => {
            assert_eq!(delegator_user_id, ahmet);
            assert_eq!(seat_role, "creditAnalyst");
        }
        other => panic!("Delegated bekleniyordu, gelen: {other:?}"),
    }
}

#[tokio::test]
async fn delegated_pool_can_claim() {
    let orgu = Uuid::new_v4();
    let ahmet = Uuid::new_v4();
    let bob = Uuid::new_v4();
    let org = MockOrg {
        roles: vec![
            (ahmet, orgu, "creditAnalyst".into()),
            (bob, orgu, "backup".into()), // Bob havuz rolünü taşır
        ],
        idents: vec![],
        delegations: vec![grant(ahmet, orgu, pool_grantee("backup"))],
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    // Bob backup — havuz grantee'ye uyar → vekaleten claim'leyebilir.
    let bob_actor = actor(orgu, bob, "backup");
    assert_eq!(
        engine
            .can_claim(&wfd, &wfes, &bob_actor, None)
            .await
            .unwrap(),
        ClaimCheck::Ok
    );
}

#[tokio::test]
async fn non_grantee_cannot_claim() {
    let orgu = Uuid::new_v4();
    let ahmet = Uuid::new_v4();
    let cem = Uuid::new_v4();
    let org = MockOrg {
        roles: vec![(ahmet, orgu, "creditAnalyst".into())],
        idents: vec![(cem, "cem".into())],
        // vekalet Ayşe'ye — Cem alıcı değil.
        delegations: vec![grant(ahmet, orgu, person_grantee("ayse"))],
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    let cem_actor = actor(orgu, cem, "clerk");
    assert_eq!(
        engine
            .can_claim(&wfd, &wfes, &cem_actor, None)
            .await
            .unwrap(),
        ClaimCheck::NotEligible
    );
}

#[tokio::test]
async fn no_active_delegation_denies() {
    let orgu = Uuid::new_v4();
    let ayse = Uuid::new_v4();
    let org = MockOrg {
        roles: vec![],
        idents: vec![(ayse, "ayse".into())],
        delegations: vec![], // süresi dolmuş/iptal → aday yok
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    let ayse_actor = actor(orgu, ayse, "clerk");
    assert_eq!(
        engine
            .can_claim(&wfd, &wfes, &ayse_actor, None)
            .await
            .unwrap(),
        ClaimCheck::NotEligible
    );
}

#[tokio::test]
async fn direct_eligibility_still_works() {
    let orgu = Uuid::new_v4();
    let analyst = Uuid::new_v4();
    let org = MockOrg {
        roles: vec![(analyst, orgu, "creditAnalyst".into())],
        idents: vec![],
        delegations: vec![],
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    let a = actor(orgu, analyst, "creditAnalyst");
    assert_eq!(
        engine.can_claim(&wfd, &wfes, &a, None).await.unwrap(),
        ClaimCheck::Ok
    );
    // Doğrudan uygun → provenance Direct (vekalet marker'ı YAZILMAZ).
    assert_eq!(
        engine.claim_decision(&wfd, &wfes, &a, None).await.unwrap(),
        AuthDecision::Direct
    );
}

#[tokio::test]
async fn delegator_not_holding_seat_denies() {
    let orgu = Uuid::new_v4();
    let ahmet = Uuid::new_v4();
    let ayse = Uuid::new_v4();
    let org = MockOrg {
        // Ahmet creditAnalyst DEĞİL — koltuk sahipliği yok → sentetik authorize düşer.
        roles: vec![],
        idents: vec![(ayse, "ayse".into())],
        delegations: vec![grant(ahmet, orgu, person_grantee("ayse"))],
    };
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst");

    let ayse_actor = actor(orgu, ayse, "clerk");
    assert_eq!(
        engine
            .can_claim(&wfd, &wfes, &ayse_actor, None)
            .await
            .unwrap(),
        ClaimCheck::NotEligible
    );
}
