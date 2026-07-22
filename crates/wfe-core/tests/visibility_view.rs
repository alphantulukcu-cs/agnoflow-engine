//! WFE-level VIEW authorization gate tests (`can_view`, spec Terminology
//! VISIBILITY/V + LISTABLE/L). Mirrors the golden-fixture MockOrg/Wfes setup
//! from `tests/pipeline.rs`.
//!
//! Covers:
//! (a) owner can always view
//! (b) a WFAH participant retains view — including on a TERMINAL WFE where
//!     both assignment and current_node are cleared by the commit
//! (c) actor authorized on the current node's c_a can view
//! (d) an unrelated actor (wrong role, not owner, not participant, not
//!     listable) CANNOT view — active or terminal
//! (e) a listable-matching actor can view; the golden fixture's second
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
use wfe_core::types::wfd_v22::WftTarget;
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::ports::{BranchState, BranchStatus, Wfes};
use wfe_core::v22::visibility::can_view;

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");
const PARALLEL_FIXTURE: &str = include_str!("fixtures/example-wfd_paralel-onay_v2_2.json");

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
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "branchClerk".into(),
    }
}

fn analyst(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "creditAnalyst".into(),
    }
}

fn manager(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "branchManager".into(),
    }
}

fn credit_dept_manager(orgu: Uuid) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: "creditDeptManager".into(),
    }
}

fn start_input(amount_requested: i64) -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": amount_requested, "purpose": "ev tadilatı"}
    })
}

fn wfes_at(node: &str, assigned: Option<Uuid>, ctx: Value) -> Wfes {
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
        dynctx: DynCtx(ctx),
        wfah,
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        assigned_to: assigned,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches: vec![],
        join_target: None,
    }
}

// ================================================================ (a) owner

#[tokio::test]
async fn owner_can_view_regardless_of_role() {
    let org = MockOrg {
        role_assigned: false,
    };
    let owner_id = Uuid::new_v4();
    // owner has no relation to node c_a nor listable — only assignment matters
    let owner = Actor {
        orgu_id: Uuid::new_v4(),
        user_id: owner_id,
        role: "branchClerk".into(),
    };
    let wfes = wfes_at("self__creditAnalyst", Some(owner_id), start_input(30_000));

    assert!(can_view(&golden(), &wfes, &owner, &org).await.unwrap());
}

// ============================================================ (b) participant

/// Terminal commit `assigned_to` ve `current_node`'u temizler; WFAH'ta eylemi
/// olan kullanıcı yine de sonucu (dynctx/end_response) görebilmeli.
#[tokio::test]
async fn wfah_participant_can_view_terminal_wfe() {
    let org = MockOrg {
        role_assigned: false,
    };
    let orgu = Uuid::new_v4();
    let participant = clerk(orgu);

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.wfah = wfes.wfah.push(
        "reject".into(),
        participant.clone(),
        Some(json!({"red_sebebi": "x"})),
    );
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    assert!(can_view(&golden(), &wfes, &participant, &org)
        .await
        .unwrap());
}

#[tokio::test]
async fn non_participant_cannot_view_terminal_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // clerk: WFAH'ta yalnız system + başka bir kullanıcı var; listable rolleri
    // (branchManager/creditDeptManager) ile de eşleşmiyor
    let outsider = clerk(orgu);

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.wfah = wfes.wfah.push("reject".into(), clerk(Uuid::new_v4()), None);
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    assert!(!can_view(&golden(), &wfes, &outsider, &org).await.unwrap());
}

/// System aktörünün (nil uuid) WFAH kayıtları katılımcı grant'i üretmez.
#[tokio::test]
async fn system_wfah_entries_do_not_grant_view() {
    let org = MockOrg {
        role_assigned: false,
    };
    let nil_viewer = Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::nil(),
        role: "x".into(),
    };

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));
    wfes.status = WfeStatus::Terminal;
    wfes.current_node = None;

    // wfah'ta yalnızca system (nil) kaydı var — nil user_id'li viewer bile geçemez
    assert!(!can_view(&golden(), &wfes, &nil_viewer, &org).await.unwrap());
}

// ================================================================ (c) node c_a

#[tokio::test]
async fn actor_authorized_on_current_node_c_a_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let a = analyst(orgu); // self__creditAnalyst node c_a = {c_orgu: self, c_r: [creditAnalyst]}
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &a, &org).await.unwrap());
}

// ================================================================ (c) unrelated actor

#[tokio::test]
async fn unrelated_actor_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
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
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // listable[0]: {c_a: {c_orgu: self, c_r: [branchManager]}} — no `when`
    let m = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(can_view(&golden(), &wfes, &m, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_true_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    // listable[1]: {when: "$ctx.credit_info.amount_requested >= 100000",
    //               c_a: {c_orgu: parent, c_r: [creditDeptManager]}}
    let d = credit_dept_manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input(150_000));

    assert!(can_view(&golden(), &wfes, &d, &org).await.unwrap());
}

#[tokio::test]
async fn listable_actor_with_when_false_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let orgu = Uuid::new_v4();
    let d = credit_dept_manager(orgu);
    // amount_requested below the listable[1] `when` threshold
    let wfes = wfes_at("self__creditAnalyst", None, start_input(30_000));

    assert!(!can_view(&golden(), &wfes, &d, &org).await.unwrap());
}

// ================================================== (c) WOR-31 parallel mode

fn paralel() -> Wfd {
    Wfd::from_json(PARALLEL_FIXTURE).unwrap()
}

fn role_actor(orgu: Uuid, role: &str) -> Actor {
    Actor {
        orgu_id: orgu,
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn branch(node: &str, status: BranchStatus, claimed_by: Option<Uuid>) -> BranchState {
    let now = chrono::Utc::now();
    BranchState {
        branch_node: node.into(),
        status,
        claimed_by,
        claimed_at: claimed_by.map(|_| now),
        entered_at: now,
    }
}

/// Fork SONRASI paralel mod: current_node NULL, join_target persist,
/// wfe-seviyesi assignment temiz — görünürlük kolların c_a'sından türemeli.
fn parallel_wfes(branches: Vec<BranchState>) -> Wfes {
    let mut wfes = wfes_at(
        "self__coordinator",
        None,
        json!({"request": {"title": "Sunucu alımı", "amount": 150000}}),
    );
    wfes.current_node = None;
    wfes.branches = branches;
    wfes.join_target = Some(WftTarget::Node {
        node: "self__resultCoordinator".into(),
    });
    wfes
}

#[tokio::test]
async fn parallel_active_branch_c_a_actor_can_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let fin = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(can_view(&paralel(), &wfes, &fin, &org).await.unwrap());
}

/// Kolu claim eden kullanıcı — rol ataması doğrulanamasa bile (ör. $ctx
/// sonradan değişip c_a eşleşmez olsa da) kol sahibi olarak görmeye devam eder.
#[tokio::test]
async fn parallel_branch_claimer_can_view_without_role() {
    let org = MockOrg {
        role_assigned: false,
    };
    let claimer = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch(
            "self__financeApprover",
            BranchStatus::Active,
            Some(claimer.user_id),
        ),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(can_view(&paralel(), &wfes, &claimer, &org).await.unwrap());
}

/// `arrived` kol artık aktif node değildir — o kolun c_a'sı VIEW grant'i üretmez.
#[tokio::test]
async fn parallel_arrived_branch_c_a_does_not_grant_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let fin = role_actor(Uuid::new_v4(), "financeApprover");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Arrived, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(!can_view(&paralel(), &wfes, &fin, &org).await.unwrap());
}

#[tokio::test]
async fn parallel_unrelated_actor_cannot_view() {
    let org = MockOrg {
        role_assigned: true,
    };
    let outsider = role_actor(Uuid::new_v4(), "requester");
    let wfes = parallel_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
    ]);

    assert!(!can_view(&paralel(), &wfes, &outsider, &org).await.unwrap());
}
