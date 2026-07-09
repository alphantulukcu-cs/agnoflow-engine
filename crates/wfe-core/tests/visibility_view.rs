//! WFE-level VIEW authorization gate tests (`can_view`, spec Terminology
//! VISIBILITY/V + LISTABLE/L). Mirrors the golden-fixture MockOrg/Wfes setup
//! from `tests/pipeline.rs`.
//!
//! Covers:
//! (a) owner can always view
//! (b) actor authorized on the current node's c_a can view
//! (c) an unrelated actor (wrong role, not owner, not listable) CANNOT view
//! (d) a listable-matching actor can view; the golden fixture's second
//!     listable entry carries a `when` guard — tested true and false.

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::Wfes;
use wfe_core::v22::visibility::can_view;

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).unwrap()
}

// ---- mock org: her ifade actor'ün anchor'ına çözülür (pipeline.rs testleriyle aynı) ----

struct MockOrg {
    role_assigned: bool,
}

#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        Ok(vec![OrgUnit {
            orgu_id: anchor,
            orgu_type: json!({"type": "branch"}),
            path: "1".into(),
        }])
    }
    async fn check_user_role(&self, _: Uuid, _: Uuid, _: &str) -> Result<bool, EngineError> {
        Ok(self.role_assigned)
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

fn clerk(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "branchClerk".into() }
}

fn analyst(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "creditAnalyst".into() }
}

fn manager(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "branchManager".into() }
}

fn credit_dept_manager(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "creditDeptManager".into() }
}

fn start_input(amount_requested: i64) -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": amount_requested, "purpose": "ev tadilatı"}
    })
}

fn wfes_at(node: &str, assigned: Option<Uuid>, ctx: Value) -> Wfes {
    let system = Actor { orgu_id: Uuid::nil(), user_id: Uuid::nil(), role: "system".into() };
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah: Wfah::empty().push("start".into(), system, None),
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        assigned_to: assigned,
        end_response: None,
    }
}

// ================================================================ (a) owner

#[tokio::test]
async fn owner_can_view_regardless_of_role() {
    let org = MockOrg { role_assigned: false };
    let owner_id = Uuid::new_v4();
    // owner has no relation to node c_a nor listable — only assignment matters
    let owner = Actor { orgu_id: Uuid::new_v4(), user_id: owner_id, role: "branchClerk".into() };
    let wfes = wfes_at("self__creditAnalyst", Some(owner_id), start_input(30_000));

    assert!(can_view(&golden(), &wfes, &owner, &org).await.unwrap());
}

// ================================================================ (b) node c_a

#[tokio::test]
async fn actor_authorized_on_current_node_c_a_can_view() {
    let org = MockOrg { role_assigned: true };
    let orgu = Uuid::new_v4();
    let a = analyst(orgu); // self__creditAnalyst node c_a = {c_orgu: self, c_r: [creditAnalyst]}
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &a, &org).await.unwrap());
}

// ================================================================ (c) unrelated actor

#[tokio::test]
async fn unrelated_actor_cannot_view() {
    let org = MockOrg { role_assigned: true };
    let orgu = Uuid::new_v4();
    // clerk: not owner, not node c_a (creditAnalyst), not in either listable rule
    // (branchManager / creditDeptManager)
    let c = clerk(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(!can_view(&golden(), &wfes, &c, &org).await.unwrap());
}

// ================================================================ (d) listable

#[tokio::test]
async fn listable_actor_without_when_can_view() {
    let org = MockOrg { role_assigned: true };
    let orgu = Uuid::new_v4();
    // listable[0]: {c_a: {c_orgu: self, c_r: [branchManager]}} — no `when`
    let m = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &m, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_true_can_view() {
    let org = MockOrg { role_assigned: true };
    let orgu = Uuid::new_v4();
    // listable[1]: {when: "$ctx.credit_info.amount_requested >= 100000",
    //               c_a: {c_orgu: parent, c_r: [creditDeptManager]}}
    let d = credit_dept_manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(150_000));

    assert!(can_view(&golden(), &wfes, &d, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_false_cannot_view() {
    let org = MockOrg { role_assigned: true };
    let orgu = Uuid::new_v4();
    let d = credit_dept_manager(orgu);
    // amount_requested below the listable[1] `when` threshold
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(!can_view(&golden(), &wfes, &d, &org).await.unwrap());
}
