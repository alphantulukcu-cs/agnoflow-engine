//! §7 pipeline entegrasyon testleri — golden fixture üzerinden uçtan uca.
//! WOR-36 (ilk-match), WOR-39 (WFT formları), WOR-41/45 (trigger retry/catch),
//! WOR-42 (terminal), WOR-46 (timeout), WOR-47 (escalation).
//! Zaman: tokio start_paused — sleep'ler gerçek beklemeden ilerler.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU32, Ordering};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::{AutoexecDef, AutoexecType, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::{ClaimCheck, Engine};
use wfe_core::v22::ports::{AutoexecRunner, CommitOutcome, ExecEnv, ExecFailure, Wfes};

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).unwrap()
}

// ---- mock org: her ifade actor'ün anchor'ına çözülür, rol ataması yapılandırılabilir ----

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

// ---- mock autoexec runner ----

enum RestBehavior {
    Ok(Value),
    AlwaysFail,
    /// timeout_seconds'tan uzun sürer — pipeline WFD.Timeout üretmeli
    Hang,
}

struct MockRunner {
    rest: RestBehavior,
    calc: Value,
    rest_calls: AtomicU32,
}

impl MockRunner {
    fn ok(score: i64, grade: &str, within_limit: bool) -> Self {
        Self {
            rest: RestBehavior::Ok(json!({"score": score, "grade": grade})),
            calc: json!({"within_limit": within_limit}),
            rest_calls: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl AutoexecRunner for MockRunner {
    async fn run(&self, def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        match def.kind {
            AutoexecType::Rest => {
                let url = def.config["url"].as_str().unwrap_or("");
                if url.contains("audit") {
                    return Ok(json!({}));
                }
                self.rest_calls.fetch_add(1, Ordering::SeqCst);
                match &self.rest {
                    RestBehavior::Ok(v) => Ok(v.clone()),
                    RestBehavior::AlwaysFail => Err(ExecFailure::failed("bağlantı hatası")),
                    RestBehavior::Hang => {
                        tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                        Ok(json!({}))
                    }
                }
            }
            AutoexecType::Calc => Ok(self.calc.clone()),
            _ => Err(ExecFailure::failed("desteklenmeyen tip")),
        }
    }
}

// ---- yardımcılar ----

fn clerk(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "branchClerk".into() }
}

fn analyst(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "creditAnalyst".into() }
}

fn manager(orgu: Uuid) -> Actor {
    Actor { orgu_id: orgu, user_id: Uuid::new_v4(), role: "branchManager".into() }
}

fn start_input() -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": 30000, "purpose": "ev tadilatı"}
    })
}

/// self__creditAnalyst node'unda bekleyen, analiste atanmış bir WFES kurar.
fn wfes_at(node: &str, assigned: Option<Uuid>, ctx: Value) -> Wfes {
    let system = Actor { orgu_id: Uuid::nil(), user_id: Uuid::nil(), role: "system".into() };
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah: Wfah::empty().push("start:start_branch_clerk".into(), system, None),
        status: WfeStatus::Active,
        current_node: Some(node.into()),
        assigned_to: assigned,
        end_response: None,
    }
}

// ================================================================ start

#[tokio::test]
async fn start_moves_to_analyst_node_with_real_wfe_id_effects() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let orgu = Uuid::new_v4();
    let actor = clerk(orgu);
    let wfe_id = Uuid::new_v4();

    let new = engine
        .start(&golden(), &actor, Uuid::nil(), &start_input(), wfe_id)
        .await
        .unwrap();

    assert_eq!(new.wfe_id, wfe_id);
    assert!(matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst"));
    // initiated_by = $actor gerçek aktörle çözülmeli
    assert_eq!(new.initial_dynctx["initiated_by"]["role"], json!("branchClerk"));
    assert_eq!(new.initial_dynctx["applicant"]["name"], json!("Ayşe Yılmaz"));
    assert_eq!(new.wfah_entries[0].action, "start:start_branch_clerk");
    assert!(new.resolved_c_a.iter().any(|c| c.role == "creditAnalyst"));
}

#[tokio::test]
async fn start_rejects_missing_required_context_field() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), &json!({"applicant": {}}), Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn start_rejects_readonly_field_in_input() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["credit_score"] = json!(999);

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), &input, Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn start_ineligible_actor_is_rejected() {
    let org = MockOrg { role_assigned: false }; // rol ataması yok
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), &start_input(), Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::StartNotEligible));
}

// ================================================================ apply — assignment

#[tokio::test]
async fn apply_on_unclaimed_wfe_is_rejected() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let err = engine
        .apply(&golden(), &wfes, &a, "analyst_approve", &json!({"credit_info": {"amount_requested": 30000}}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotClaimed));
}

#[tokio::test]
async fn apply_by_non_owner_is_rejected() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());

    let err = engine
        .apply(&golden(), &wfes, &a, "analyst_approve", &json!({"credit_info": {"amount_requested": 30000}}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotOwner));
}

// ================================================================ apply — happy path

#[tokio::test(start_paused = true)]
async fn analyst_approve_within_limit_reaches_terminal_approved() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(&golden(), &wfes, &a, "analyst_approve",
               &json!({"credit_info": {"amount_requested": 30000}}))
        .await
        .unwrap();

    // trigger effects staged
    assert_eq!(commit.new_dynctx["credit_score"], json!(750));
    assert_eq!(commit.new_dynctx["credit_grade"], json!("A"));
    assert_eq!(commit.new_dynctx["within_limit"], json!(true));
    // terminal + $-string resolve (M9/WOR-42)
    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("terminal bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(end_response["status"], json!("approved"));
    assert_eq!(end_response["amount_granted"], json!(30000));
    assert_eq!(end_response["applicant_name"], json!("Ayşe Yılmaz"));
    // WFAH: action + 3 trigger
    let actions: Vec<&str> = commit.wfah_entries.iter().map(|e| e.action.as_str()).collect();
    assert_eq!(
        actions,
        vec!["analyst_approve", "trigger:kredi_skoru_getir", "trigger:limit_kontrol", "trigger:audit_log"]
    );
}

#[tokio::test(start_paused = true)]
async fn analyst_approve_over_limit_routes_to_branch_manager() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(650, "C", false); // skor düşük → within_limit false
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(&golden(), &wfes, &a, "analyst_approve",
               &json!({"credit_info": {"amount_requested": 90000}}))
        .await
        .unwrap();

    assert!(matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "default branch şube müdürüne gitmeli: {:?}", commit.outcome);
    assert!(commit.resolved_c_a.iter().any(|c| c.role == "branchManager"));
}

// ================================================================ trigger retry / catch

#[tokio::test(start_paused = true)]
async fn failing_score_fetch_is_retried_then_caught_and_routed() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner {
        rest: RestBehavior::AlwaysFail,
        calc: json!({"within_limit": true}),
        rest_calls: AtomicU32::new(0),
    };
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(&golden(), &wfes, &a, "analyst_approve",
               &json!({"credit_info": {"amount_requested": 30000}}))
        .await
        .unwrap();

    // ASL: max_attempts=3 retry → toplam 4 çağrı
    assert_eq!(runner.rest_calls.load(Ordering::SeqCst), 4);
    // catch effects staged
    assert_eq!(commit.new_dynctx["score_fetch_failed"], json!(true));
    // limit_kontrol when=false → atlanmış olmalı (within_limit yok)
    assert!(commit.new_dynctx.get("within_limit").is_none());
    // wft ilk condition → şube müdürü
    assert!(matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"));
    // handled trigger WFAH'ta işaretli
    let trig = commit.wfah_entries.iter()
        .find(|e| e.action == "trigger:kredi_skoru_getir").unwrap();
    assert_eq!(trig.input.as_ref().unwrap()["handled"], json!(true));
}

#[tokio::test(start_paused = true)]
async fn hanging_autoexec_times_out_and_is_caught() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner {
        rest: RestBehavior::Hang, // 10s timeout'u aşar
        calc: json!({"within_limit": true}),
        rest_calls: AtomicU32::new(0),
    };
    let engine = Engine { org: &org, exec: &runner };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(&golden(), &wfes, &a, "analyst_approve",
               &json!({"credit_info": {"amount_requested": 30000}}))
        .await
        .unwrap();

    // WFD.Timeout retry listesinde → 4 deneme, sonra catch
    assert_eq!(runner.rest_calls.load(Ordering::SeqCst), 4);
    assert_eq!(commit.new_dynctx["score_fetch_failed"], json!(true));
    let trig = commit.wfah_entries.iter()
        .find(|e| e.action == "trigger:kredi_skoru_getir").unwrap();
    assert_eq!(trig.input.as_ref().unwrap()["error"], json!("WFD.Timeout"));
}

// ================================================================ manager decide

#[tokio::test(start_paused = true)]
async fn manager_reject_takes_default_terminal() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let mut ctx = start_input();
    ctx["score_fetch_failed"] = json!(true);
    let wfes = wfes_at("self__branchManager", Some(m.user_id), ctx);

    let commit = engine
        .apply(&golden(), &wfes, &m, "manager_decide",
               &json!({"manager_decision": "reject"}))
        .await
        .unwrap();

    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("terminal bekleniyordu");
    };
    assert_eq!(end_response["status"], json!("rejected"));
    assert_eq!(end_response["amount_granted"], json!(0));
    // input declared path ctx'e yazılmış olmalı (§7.5)
    assert_eq!(commit.new_dynctx["manager_decision"], json!("reject"));
}

#[tokio::test(start_paused = true)]
async fn manager_approve_condition_hits_terminal_approved() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let commit = engine
        .apply(&golden(), &wfes, &m, "manager_decide",
               &json!({"manager_decision": "approve"}))
        .await
        .unwrap();

    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("terminal bekleniyordu");
    };
    assert_eq!(end_response["status"], json!("approved"));
}

#[tokio::test]
async fn undeclared_input_path_is_rejected() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(&golden(), &wfes, &m, "manager_decide",
               &json!({"manager_decision": "approve", "credit_score": 999}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn missing_required_input_is_rejected() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(&golden(), &wfes, &m, "manager_decide", &json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

// ================================================================ ilk-match seçimi (M2)

fn first_match_wfd() -> Wfd {
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    // manager_decide için iki transition: ilki when'li (false olacak), ikincisi fallback
    let base = v["transitions"][1].clone();
    let mut guarded = base.clone();
    guarded["id"] = json!("t_guarded");
    guarded["when"] = json!("$ctx.credit_info.amount_requested >= 1000000");
    guarded["wft"] = json!({"node": "parent__creditDeptManager"});
    let mut fallback = base;
    fallback["id"] = json!("t_fallback");
    fallback["when"] = json!("$ctx.credit_info.amount_requested < 1000000");
    v["transitions"][1] = guarded;
    v["transitions"].as_array_mut().unwrap().push(fallback);
    Wfd::from_value(v).unwrap()
}

#[tokio::test(start_paused = true)]
async fn first_matching_when_wins_in_array_order() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    // 30000 < 1000000 → t_guarded'ın when'i false → t_fallback seçilmeli
    let commit = engine
        .apply(&first_match_wfd(), &wfes, &m, "manager_decide",
               &json!({"manager_decision": "approve"}))
        .await
        .unwrap();
    assert!(matches!(&commit.outcome, CommitOutcome::Terminal { .. }),
        "fallback transition'ın wft'si (terminal) seçilmeliydi: {:?}", commit.outcome);
}

// ================================================================ NoConditionMatched (M3)

#[tokio::test(start_paused = true)]
async fn conditional_without_default_and_no_match_errors() {
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["transitions"][1]["wft"] = json!({
        "conditions": [{"when": "$action.input.manager_decision == 'never'", "terminal": "terminal_approved"}]
    });
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(&wfd, &wfes, &m, "manager_decide", &json!({"manager_decision": "approve"}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NoConditionMatched), "{err}");
}

// ================================================================ claim

#[tokio::test]
async fn claim_checks() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden();

    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    assert_eq!(engine.can_claim(&wfd, &wfes, &a).await.unwrap(), ClaimCheck::Ok);

    // yanlış rol → uygun değil
    let c = clerk(Uuid::new_v4());
    assert_eq!(engine.can_claim(&wfd, &wfes, &c).await.unwrap(), ClaimCheck::NotEligible);

    // zaten claim edilmiş
    let claimed = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    assert_eq!(engine.can_claim(&wfd, &claimed, &a).await.unwrap(), ClaimCheck::AlreadyClaimed);
}

// ================================================================ possible actions

#[tokio::test]
async fn owner_sees_available_actions() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let actions = engine.possible_actions(&golden(), &wfes, &m).await.unwrap();
    assert_eq!(actions, vec!["manager_decide"]);

    // owner olmayan boş liste alır
    let other = manager(Uuid::new_v4());
    let actions = engine.possible_actions(&golden(), &wfes, &other).await.unwrap();
    assert!(actions.is_empty());
}

// ================================================================ escalation (M6)

#[tokio::test]
async fn escalation_fires_after_sla_and_moves_wfe() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    // P3D dolmadan due değil
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, entered_at + Duration::days(2)).unwrap(),
        None
    );
    // P3D sonrası due
    let now = entered_at + Duration::days(3) + Duration::seconds(1);
    assert_eq!(engine.due_escalation(&wfd, &wfes, now).unwrap(), Some(0));

    // fire → şube müdürüne taşınır, effects uygulanır (assigned olsa bile çalışır)
    let commit = engine.fire_escalation(&wfd, &wfes, 0, now).await.unwrap();
    assert!(matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"));
    assert!(commit.new_dynctx["internal_notes"].as_str().unwrap().contains("SLA"));
    assert_eq!(commit.wfah_entries[0].action, "escalate:self__creditAnalyst:0");
    assert_eq!(commit.wfah_entries[0].actor.role, "system");
}

#[tokio::test]
async fn fired_escalation_step_does_not_refire() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden();
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    // adım ateşlenmiş gibi işaretle
    let system = Actor { orgu_id: Uuid::nil(), user_id: Uuid::nil(), role: "system".into() };
    wfes.wfah = wfes.wfah.push("escalate:self__creditAnalyst:0".into(), system, None);

    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;
    let now = entered_at + Duration::days(10);
    assert_eq!(engine.due_escalation(&wfd, &wfes, now).unwrap(), None,
        "ateşlenen adım tekrar due olmamalı");
}

// ================================================================ root timeout (M5)

#[tokio::test]
async fn root_timeout_fires_engine_defined_fail() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden(); // timeout: P30D
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    let started = wfes.wfah.entries().first().unwrap().applied_at;

    assert!(!engine.root_timeout_due(&wfd, &wfes, started + Duration::days(29)).unwrap());
    let now = started + Duration::days(30) + Duration::seconds(1);
    assert!(engine.root_timeout_due(&wfd, &wfes, now).unwrap());

    let commit = engine.fire_root_timeout(&wfd, &wfes, now).unwrap();
    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("terminal bekleniyordu");
    };
    assert_eq!(end_response["error"], json!("WFD.Timeout"));
    assert_eq!(commit.wfah_entries[0].action, "timeout:root");
}

// ================================================================ terminal WFE korunur

#[tokio::test]
async fn terminal_wfe_rejects_actions() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let mut wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());
    wfes.status = WfeStatus::Terminal;

    let err = engine
        .apply(&golden(), &wfes, &m, "manager_decide", &json!({"manager_decision": "approve"}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeTerminal));
    assert_eq!(engine.can_claim(&golden(), &wfes, &m).await.unwrap(), ClaimCheck::Terminal);
}
