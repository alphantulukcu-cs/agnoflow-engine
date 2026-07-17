//! WOR-31 T4 — `sim.rs` (store'suz simülasyon) uçtan uca fork/join testi.
//!
//! `crates/wfe/tests/fork_join.rs`'in store'lu (WfeExecutor + WfeStore CAS/retry)
//! karşılığı; burada engine DOĞRUDAN `SimState` üzerinden sürülür — tıpkı
//! `routes/simulate.rs`'in yaptığı gibi, HTTP katmanı ve persistence YOK.
//! Tek-thread'li olduğundan `BranchArrived`/`JoinComplete` doğrulaması trivially
//! doğrudur (adapter'ın FOR UPDATE + CAS'ının karşılığı yok — bkz. `sim.rs`
//! `apply_branch_outcome` yorumu).

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use wf_wfe::executor::possible_actions_for;
use wf_wfe::sim::SimState;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::wfd_v22::{AutoexecDef, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure};
use wfe_core::EngineError;

const PARALLEL_FIXTURE: &str =
    include_str!("../../wfe-core/tests/fixtures/example-wfd_paralel-onay_v2_2.json");

// ---- mock'lar (fork_join.rs / pipeline.rs kalıbı — authorize anchor = actor.orgu_id) ----

struct MockOrg;

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
        Ok(true)
    }
    async fn orgtnt_for_orgu(&self, _: Uuid) -> Result<Uuid, EngineError> {
        Ok(Uuid::nil())
    }
}

struct MockRunner;

#[async_trait]
impl AutoexecRunner for MockRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<serde_json::Value, ExecFailure> {
        Ok(json!({}))
    }
}

static MOCK_ORG: MockOrg = MockOrg;
static MOCK_RUNNER: MockRunner = MockRunner;

fn engine() -> Engine<'static> {
    Engine {
        org: &MOCK_ORG,
        exec: &MOCK_RUNNER,
    }
}

fn actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn wfd() -> Wfd {
    Wfd::from_json(PARALLEL_FIXTURE).unwrap()
}

/// start (requester) → self__coordinator; start_review (coordinator) → fork.
/// Sim'de claim akışı atlanır — `to_wfes(Some(actor.user_id))` her zaman o
/// aktörü çağrılan node/kolun sahibi sayar (bkz. `sim.rs::to_wfes`).
async fn fork_setup(eng: &Engine<'_>, wfd: &Wfd) -> SimState {
    let requester = actor("requester");
    let start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    let new = eng
        .start(
            wfd,
            &requester,
            Uuid::nil(),
            None,
            &start_input,
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    let mut sim_state = SimState::from_new_wfe(&new);
    assert_eq!(sim_state.current_node.as_deref(), Some("self__coordinator"));
    assert!(sim_state.branches.is_empty());
    assert!(sim_state.join_target.is_none());

    let coord = actor("coordinator");
    let wfes = sim_state.to_wfes(Some(coord.user_id));
    let commit = eng
        .apply(wfd, &wfes, &coord, "start_review", &json!({}), None)
        .await
        .unwrap();
    sim_state.apply_commit(&commit);

    assert_eq!(sim_state.current_node, None, "fork sonrası wfe-seviyesi node yok");
    assert!(sim_state.join_target.is_some(), "paralel moda girildi");
    assert_eq!(sim_state.branches.len(), 3);
    sim_state
}

/// Uçtan uca mutlu yol: fork → 3 kol approve (2 ara-varış + son join) → join
/// node'a promote → finalize → terminal_approved. `crates/wfe/tests/fork_join.rs`
/// `happy_path_fork_join_finalize`'in sim (store'suz) karşılığı.
#[tokio::test]
async fn sim_happy_path_fork_join_finalize() {
    let wfd = wfd();
    let eng = engine();
    let mut sim_state = fork_setup(&eng, &wfd).await;

    let branches = [
        ("financeApprover", "self__financeApprover"),
        ("legalApprover", "self__legalApprover"),
        ("hrApprover", "self__hrApprover"),
    ];

    // ilk iki approve → BranchArrived (paralel mod sürer)
    for (role, node) in &branches[..2] {
        let a = actor(role);
        let wfes = sim_state.to_wfes(Some(a.user_id));
        let commit = eng
            .apply(&wfd, &wfes, &a, "approve", &json!({}), Some(node))
            .await
            .unwrap();
        sim_state.apply_commit(&commit);
        assert_eq!(sim_state.current_node, None);
        assert!(sim_state.join_target.is_some(), "hâlâ paralel");
    }
    assert_eq!(
        sim_state
            .branches
            .iter()
            .filter(|b| b.status == wfe_core::v22::ports::BranchStatus::Active)
            .count(),
        1,
        "2 kol arrived, 1 aktif kaldı"
    );

    // son approve → JoinComplete → MoveTo join node
    let (role, node) = branches[2];
    let a = actor(role);
    let wfes = sim_state.to_wfes(Some(a.user_id));
    let commit = eng
        .apply(&wfd, &wfes, &a, "approve", &json!({}), Some(node))
        .await
        .unwrap();
    sim_state.apply_commit(&commit);

    assert_eq!(sim_state.current_node.as_deref(), Some("self__resultCoordinator"));
    assert!(sim_state.join_target.is_none(), "paralel mod bitti");
    assert!(sim_state.branches.is_empty(), "kollar temizlendi");
    assert!(
        sim_state.wfah.iter().any(|e| e.action == "_join"),
        "_join marker beklendi"
    );
    assert!(sim_state.wfah.iter().any(|e| e.action == "_fork"));
    assert_eq!(
        sim_state.wfah.iter().filter(|e| e.action == "_branch_arrived").count(),
        3,
        "her varış (ara + son) _branch_arrived üretmeli"
    );

    // finalize → terminal_approved
    let rc = actor("resultCoordinator");
    let wfes = sim_state.to_wfes(Some(rc.user_id));
    let commit = eng
        .apply(&wfd, &wfes, &rc, "finalize", &json!({}), None)
        .await
        .unwrap();
    sim_state.apply_commit(&commit);

    assert_eq!(sim_state.status, WfeStatus::Terminal);
    let end = sim_state.end_response.expect("end_response");
    assert_eq!(end.get("status").and_then(|v| v.as_str()), Some("approved"));
}

/// Bir kolun reddi TÜM WFE'yi terminal_rejected'a alır; kardeş kollar iptal
/// (`_branch_cancelled` marker'ları engine tarafından staged). `fork_join.rs`
/// `branch_reject_terminates_and_cancels_siblings`'in sim karşılığı.
#[tokio::test]
async fn sim_branch_reject_terminates_and_cancels_siblings() {
    let wfd = wfd();
    let eng = engine();
    let mut sim_state = fork_setup(&eng, &wfd).await;

    let legal = actor("legalApprover");
    let wfes = sim_state.to_wfes(Some(legal.user_id));
    let commit = eng
        .apply(
            &wfd,
            &wfes,
            &legal,
            "reject",
            &json!({}),
            Some("self__legalApprover"),
        )
        .await
        .unwrap();
    sim_state.apply_commit(&commit);

    assert_eq!(sim_state.status, WfeStatus::Terminal);
    assert_eq!(sim_state.current_node, None);
    assert!(sim_state.join_target.is_none(), "paralel mod bitti");
    assert!(sim_state.branches.is_empty(), "kollar temizlendi");

    let end = sim_state.end_response.expect("end_response");
    assert_eq!(end.get("status").and_then(|v| v.as_str()), Some("rejected"));

    // reddeden kol dışındaki 2 kardeş kol için `_branch_cancelled` marker'ı
    let cancelled: Vec<_> = sim_state
        .wfah
        .iter()
        .filter(|e| e.action == "_branch_cancelled")
        .collect();
    assert_eq!(cancelled.len(), 2, "reddeden kol hariç 2 kardeş iptal edilmeli");
}

/// Aksiyon ≥2 aktif kolun transition'ıyla eşleşir ve `node` hint verilmezse
/// `AmbiguousAction` — üç kol da aynı `approve`/`reject` action adını taşır.
#[tokio::test]
async fn sim_apply_without_node_hint_is_ambiguous() {
    let wfd = wfd();
    let eng = engine();
    let sim_state = fork_setup(&eng, &wfd).await;

    let a = actor("financeApprover");
    let wfes = sim_state.to_wfes(Some(a.user_id));
    let err = eng
        .apply(&wfd, &wfes, &a, "approve", &json!({}), None)
        .await
        .unwrap_err();
    match err {
        EngineError::AmbiguousAction { action, candidates } => {
            assert_eq!(action, "approve");
            assert_eq!(candidates.len(), 3);
        }
        other => panic!("AmbiguousAction bekleniyordu, geldi: {other:?}"),
    }
}

/// `possible_actions_for` paralel modda TÜM aktif kollar için birleşim döner,
/// her öğe kendi kol `node`'uyla etiketli (T4 — `routes/simulate.rs` bunu
/// kullanır: sim adım yanıtlarındaki `possible_actions`).
#[tokio::test]
async fn sim_possible_actions_unions_active_branches() {
    let wfd = wfd();
    let eng = engine();
    let sim_state = fork_setup(&eng, &wfd).await;

    // bypass: aktör her aktif kolun claimed_by'ı sayılır — üçü de "approve"/"reject" sunar
    let a = actor("anyone");
    let wfes = sim_state.to_wfes(Some(a.user_id));
    let actions = possible_actions_for(&eng, &wfd, &wfes, &a).await.unwrap();

    let nodes: std::collections::BTreeSet<_> = actions
        .iter()
        .filter(|pa| pa.action == "approve")
        .filter_map(|pa| pa.node.clone())
        .collect();
    assert_eq!(
        nodes,
        std::collections::BTreeSet::from([
            "self__financeApprover".to_string(),
            "self__legalApprover".to_string(),
            "self__hrApprover".to_string(),
        ]),
        "approve üç kolda da mümkün olmalı, her biri kendi node'uyla"
    );
}
