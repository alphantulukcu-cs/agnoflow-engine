//! `wf_wfe::scenario::run` uçtan uca — ağsız, store'suz (mock OrgPort/AutoexecRunner).
//!
//! Mock kalıbı `crates/wfe/tests/sim_fork_join.rs`'ten alınmıştır: authorize
//! anchor'ı aktörün kendi birimidir, rol denetimi her zaman doğrudur. Testler
//! ORGU/rol çözümünü değil, KOŞUCUNUN kendisini sınar.

use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use wf_wfe::scenario::{run, Expect, Scenario, ScenarioActor, ScenarioStep};
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::wfd_v22::{AutoexecDef, Wfd};
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure};
use wfe_core::EngineError;

const GOLDEN: &str = include_str!("../../wfe-core/tests/fixtures/kredi-basvuru.golden.json");

struct MockOrg;
#[async_trait]
impl OrgPort for MockOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        _e: &str,
        _t: Uuid,
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
    async fn run(&self, _d: &AutoexecDef, _e: &ExecEnv) -> Result<serde_json::Value, ExecFailure> {
        Ok(json!({}))
    }
}

static MOCK_ORG: MockOrg = MockOrg;
static MOCK_RUNNER: MockRunner = MockRunner;

fn engine() -> Engine<'static> {
    Engine {
        org: &MOCK_ORG,
        exec: &MOCK_RUNNER,
        env: Default::default(),
    }
}

fn sc_actor(role: &str) -> ScenarioActor {
    ScenarioActor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn fallback() -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: "branchClerk".into(),
    }
}

/// Golden fixture'ın tek start kuralı: `create_application` (branchClerk).
fn base_scenario() -> Scenario {
    Scenario {
        id: "s1".into(),
        name: "test".into(),
        path: String::new(),
        description: None,
        environment: None,
        start_actor: Some(sc_actor("branchClerk")),
        start_action: None,
        start_input: json!({
            "applicant": { "name": "Ayşe" },
            "credit_info": { "amount": 100000 }
        }),
        steps: vec![],
        expect: None,
    }
}

fn golden() -> (Wfd, serde_json::Value) {
    (
        Wfd::from_json(GOLDEN).unwrap(),
        serde_json::from_str(GOLDEN).unwrap(),
    )
}

/// Beklentisiz senaryo start atıp durur ve GEÇER — koşucu "hata yoksa ok".
#[tokio::test]
async fn scenario_without_expectations_passes_after_start() {
    let (wfd, json_doc) = golden();
    let res = run(&engine(), &wfd, &json_doc, &base_scenario(), None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 0);
    assert!(!res.terminal, "start sonrası akış aktif olmalı");
}

/// Aktörü olmayan senaryo, fallback verilmezse KALIR (panik değil).
#[tokio::test]
async fn scenario_without_actor_and_without_fallback_fails() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.start_actor = None;
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("aktör"), "{:?}", res.failures);
}

/// Fallback verilirse aktörsüz senaryo koşar.
#[tokio::test]
async fn fallback_actor_is_used_when_the_scenario_has_none() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.start_actor = None;
    let res = run(&engine(), &wfd, &json_doc, &s, Some(&fallback())).await;
    assert!(res.ok, "{:?}", res.failures);
}

/// Karşılanmayan terminal beklentisi failure üretir, koşuyu patlatmaz.
#[tokio::test]
async fn unmet_terminal_expectation_is_a_failure_not_an_error() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.expect = Some(Expect {
        terminal: Some("YokBoyleTerminal".into()),
        context_contains: None,
    });
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok);
    assert_eq!(res.failures.len(), 1);
}

/// Var olmayan aksiyon: motor hatası failure'a çevrilir, sonraki adımlar atlanır.
#[tokio::test]
async fn engine_error_stops_the_run_and_becomes_a_failure() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.steps = vec![
        ScenarioStep::Action {
            action: "boyle_bir_aksiyon_yok".into(),
            actor: Some(sc_actor("creditAnalyst")),
            input: json!({}),
            node: None,
        },
        ScenarioStep::Action {
            action: "ikinci".into(),
            actor: Some(sc_actor("creditAnalyst")),
            input: json!({}),
            node: None,
        },
    ];
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok);
    assert_eq!(
        res.steps_executed, 0,
        "hatalı adım sayılmaz ve sonrası koşulmaz"
    );
    assert!(res.failures[0].contains("Adım 1"), "{:?}", res.failures);
}

/// `startAction` var olmayan bir adı gösterirse senaryo kalır.
#[tokio::test]
async fn unknown_start_action_fails_the_scenario() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.start_action = Some("boyle_bir_start_yok".into());
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].starts_with("start:"), "{:?}", res.failures);
}

/// Bekleyen çağrı yokken `call_return` adımı senaryoyu kaldırır.
#[tokio::test]
async fn call_return_without_a_waiting_call_fails_the_scenario() {
    let (wfd, json_doc) = golden();
    let mut s = base_scenario();
    s.steps = vec![ScenarioStep::CallReturn {
        call_return: wf_wfe::scenario::CallReturn {
            status: "completed".into(),
            result: None,
        },
    }];
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("çağrı"), "{:?}", res.failures);
}
