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
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{AutoexecDef, AutoexecType, ClaimTimeout, EscalationStep, Wfd, Wft};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::{ClaimCheck, ClaimTimeoutOutcome, Engine};
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
        claimed_at: assigned.map(|_| created_at),
        created_at,
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
        .start(&golden(), &actor, Uuid::nil(), None, &start_input(), wfe_id, None)
        .await
        .unwrap();

    assert_eq!(new.wfe_id, wfe_id);
    assert!(matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst"));
    // initiated_by = $actor gerçek aktörle çözülmeli
    assert_eq!(new.initial_dynctx["initiated_by"]["role"], json!("branchClerk"));
    assert_eq!(new.initial_dynctx["applicant"]["name"], json!("Ayşe Yılmaz"));
    // M16: WFAH start kaydı gerçek action adını taşır (rezerve "start" değil)
    assert_eq!(new.wfah_entries[0].action, "create_application");
    assert!(new.resolved_c_a.iter().any(|c| c.role == "creditAnalyst"));
}

#[tokio::test]
async fn start_rejects_missing_required_context_field() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), None, &json!({"applicant": {}}), Uuid::new_v4(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn start_rejects_undeclared_input_path() {
    // §7.5 simetrisi: start action'ın input tanımında olmayan yol ctx'e sızamaz.
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["status"] = json!("approved"); // bildirilmemiş alan — enjeksiyon denemesi

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), None, &input, Uuid::new_v4(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidInput(m) if m.contains("tanımlı değil")),
        "{err}"
    );
}

#[tokio::test]
async fn start_rejects_missing_required_action_input() {
    // start action'ın input.required'ı transition'lardaki gibi zorunludur.
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());
    let input = json!({"applicant": {"name": "Ayşe"}}); // credit_info yok

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), None, &input, Uuid::new_v4(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidInput(m) if m.contains("zorunlu input")),
        "{err}"
    );
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
        .start(&golden(), &actor, Uuid::nil(), None, &input, Uuid::new_v4(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn start_with_named_action_selects_matching_rule() {
    // M16: start.action gerçek ad — istemci action adı verirse yalnız o kural aday olur.
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(&golden(), &actor, Uuid::nil(), Some("create_application"), &start_input(), Uuid::new_v4(), None)
        .await
        .unwrap();
    assert_eq!(new.wfah_entries[0].action, "create_application");
}

#[tokio::test]
async fn start_with_unknown_action_is_not_eligible() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), Some("ghost_action"), &start_input(), Uuid::new_v4(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::StartNotEligible));
}

#[tokio::test]
async fn start_ineligible_actor_is_rejected() {
    let org = MockOrg { role_assigned: false }; // rol ataması yok
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(&golden(), &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), None)
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

// #4 — çok-adımlı aynı-node escalation: her adımın `after`'ı NODE GİRİŞİNDEN ölçülür,
// bir önceki adımın marker'ından değil. Adım 0 (P3D), node'a girişten 3 gün SONRA
// ateşlenmiş olsa bile adım 1 (P5D) yine node girişinden +5 günde due olmalı (+8'de değil).
#[tokio::test]
async fn multi_step_escalation_measures_after_from_node_entry() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };

    // Golden'a creditAnalyst node'una ikinci bir escalation adımı (P5D) ekle.
    let mut wfd = golden();
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P5D".into(),
            wfes_effects: None,
            wft: Some(Wft::Node { node: "self__branchManager".into() }),
            terminate: None,
        });

    let t0 = Utc::now();
    let system = Actor { orgu_id: Uuid::nil(), user_id: Uuid::nil(), role: "system".into() };
    // Kontrollü WFAH: node girişi T0; adım 0 marker'ı T0+3g (gün sonra ateşlendi).
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.wfah = Wfah(vec![
        WfahEntry { seq: 1, action: "start".into(), actor: system.clone(), input: None, applied_at: t0 },
        WfahEntry { seq: 2, action: "escalate:self__creditAnalyst:0".into(), actor: system.clone(), input: None, applied_at: t0 + Duration::days(3) },
    ]);

    // Adım 0 ateşlendi; adım 1 (P5D) node girişinden +4 günde henüz due DEĞİL.
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, t0 + Duration::days(4)).unwrap(),
        None,
        "adım 1 node girişinden +5g'de due olmalı, +4g'de değil",
    );
    // Node girişinden +5 gün + 1sn: adım 1 due (marker'dan ölçülseydi +8g olurdu).
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, t0 + Duration::days(5) + Duration::seconds(1)).unwrap(),
        Some(1),
        "adım 1'in `after`'ı NODE GİRİŞİNDEN ölçülmeli (marker'dan değil)",
    );
}

// ---- start node yeniden girilebilir (2026-07-16): start node artık normal bir
// mid-flow hedefi/ara-durak olabilir; escalation orada da normal işler. ----

#[tokio::test]
async fn start_wft_targeting_own_from_node_lands_there() {
    // Bir start rule kendi `from`'unu wft hedefi seçebilir (örn. memur başlatır,
    // akış müdür node'una gider; müdür başlatınca memur node'una — burada
    // sadeleştirilmiş biçimde: start.wft kendi from'unu hedefliyor).
    let mut wfd = golden();
    wfd.start[0].wft = Wft::Node { node: "type_branch__branchClerk".into() };

    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(&wfd, &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), None)
        .await
        .unwrap();
    assert!(
        matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "type_branch__branchClerk"),
        "{:?}", new.outcome
    );
}

#[tokio::test]
async fn escalation_fires_normally_at_start_node() {
    // start node'a mid-flow'da (wft ile) girilen bir WFE, orada normal node gibi
    // escalation taşıyabilir ve SLA aşımında normal şekilde ateşlenir.
    let mut wfd = golden();
    wfd.nodes
        .get_mut("type_branch__branchClerk")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P1D".into(),
            wfes_effects: None,
            wft: Some(Wft::Node { node: "self__creditAnalyst".into() }),
            terminate: None,
        });

    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfes = wfes_at("type_branch__branchClerk", None, start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    assert_eq!(
        engine.due_escalation(&wfd, &wfes, entered_at + Duration::hours(12)).unwrap(),
        None
    );
    let now = entered_at + Duration::days(1) + Duration::seconds(1);
    assert_eq!(engine.due_escalation(&wfd, &wfes, now).unwrap(), Some(0));

    let commit = engine.fire_escalation(&wfd, &wfes, 0, now).await.unwrap();
    assert!(matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst"));
}

// ================================================================ SLA-3 deadline (2026-07-16)

#[tokio::test]
async fn deadline_due_fires_terminated_not_error() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    let deadline = wfes.created_at + Duration::days(30);
    wfes.deadline = Some(deadline);

    assert!(!engine.deadline_due(&wfes, deadline - Duration::seconds(1)));
    let now = deadline + Duration::seconds(1);
    assert!(engine.deadline_due(&wfes, now));

    let commit = engine.fire_deadline_timeout(&wfes, now);
    // SLA ihlali `error` DEĞİL, `terminated`dır — Failed'ten ayrı (2026-07-16 sözleşmesi).
    let CommitOutcome::Terminated { end_response } = &commit.outcome else {
        panic!("Terminated bekleniyordu");
    };
    assert_eq!(end_response["reason"], json!("SLA.Deadline"));
    assert_eq!(commit.wfah_entries[0].action, "timeout:deadline");

    // deadline yoksa hiçbir zaman due değil
    wfes.deadline = None;
    assert!(!engine.deadline_due(&wfes, now + Duration::days(999)));

    // terminal-class WFE'de asla due sayılmaz
    wfes.deadline = Some(deadline);
    wfes.status = WfeStatus::Terminated;
    assert!(!engine.deadline_due(&wfes, now));
}

#[tokio::test]
async fn start_resolves_deadline_and_rejects_when_exceeding_wfd_timeout() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden(); // timeout: P30D
    let actor = clerk(Uuid::new_v4());

    // deadline > wfd.timeout → InvalidInput hard-reject
    let err = engine
        .start(&wfd, &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), Some("P40D"))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");

    // deadline ≤ timeout → kabul, mutlak deadline start anından itibaren çözülür
    let before = Utc::now();
    let new = engine
        .start(&wfd, &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), Some("P10D"))
        .await
        .unwrap();
    let deadline = new.deadline.expect("deadline verildiğinde resolve edilmeli");
    assert!(deadline >= before + Duration::days(10) && deadline <= Utc::now() + Duration::days(10));

    // deadline verilmedi, wfd.timeout var → wfd.timeout kullanılır
    let new = engine
        .start(&wfd, &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), None)
        .await
        .unwrap();
    let deadline = new.deadline.expect("wfd.timeout varken deadline resolve edilmeli");
    assert!(deadline >= before + Duration::days(30) && deadline <= Utc::now() + Duration::days(30));
}

#[tokio::test]
async fn start_without_deadline_or_timeout_leaves_deadline_null() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine { org: &org, exec: &runner };
    let mut wfd = golden();
    wfd.timeout = None;
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(&wfd, &actor, Uuid::nil(), None, &start_input(), Uuid::new_v4(), None)
        .await
        .unwrap();
    assert!(new.deadline.is_none());
}

// ================================================================ SLA-1 claim timeout (2026-07-16)

/// self__creditAnalyst'e claim_timeout ekleyen golden varyantı.
fn golden_with_claim_timeout(after: &str, wft: Option<&str>) -> Wfd {
    let mut wfd = golden();
    wfd.nodes.get_mut("self__creditAnalyst").unwrap().claim_timeout = Some(ClaimTimeout {
        after: after.into(),
        wft: wft.map(String::from),
    });
    wfd
}

#[tokio::test]
async fn claim_timeout_due_without_wft_releases_claim() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden_with_claim_timeout("PT2H", None);
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);

    assert!(!engine.claim_timeout_due(&wfd, &wfes, claimed_at + Duration::hours(1)).unwrap());
    let now = claimed_at + Duration::hours(2) + Duration::seconds(1);
    assert!(engine.claim_timeout_due(&wfd, &wfes, now).unwrap());

    match engine.fire_claim_timeout(&wfd, &wfes, now).await.unwrap() {
        ClaimTimeoutOutcome::Release(entry) => {
            assert_eq!(entry.action, "claim_timeout:self__creditAnalyst");
            assert_eq!(entry.actor.role, "system");
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

#[tokio::test]
async fn claim_timeout_due_with_wft_moves_like_escalation() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden_with_claim_timeout("PT1H", Some("self__branchManager"));
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);
    let now = claimed_at + Duration::hours(1) + Duration::seconds(1);

    match engine.fire_claim_timeout(&wfd, &wfes, now).await.unwrap() {
        ClaimTimeoutOutcome::Move(commit) => {
            assert!(matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"));
            assert_eq!(commit.wfah_entries[0].action, "claim_timeout:self__creditAnalyst");
        }
        ClaimTimeoutOutcome::Release(_) => panic!("wft varken Move bekleniyordu"),
    }
}

#[tokio::test]
async fn claim_timeout_not_due_without_claim() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let wfd = golden_with_claim_timeout("PT1H", None);
    // hiç claim edilmemiş (claimed_at None) — asla due olmaz
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    assert!(!engine.claim_timeout_due(&wfd, &wfes, wfes.created_at + Duration::days(1)).unwrap());
}

// ================================================================ SLA-2 escalation terminate (2026-07-16)

#[tokio::test]
async fn escalation_terminate_true_ends_instance_as_terminated() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let mut wfd = golden();
    {
        let esc = &mut wfd.nodes.get_mut("self__creditAnalyst").unwrap().escalation[0];
        esc.wft = None;
        esc.terminate = Some(true);
    }
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    let now = wfes.created_at + Duration::days(3) + Duration::seconds(1);

    let commit = engine.fire_escalation(&wfd, &wfes, 0, now).await.unwrap();
    let CommitOutcome::Terminated { end_response } = &commit.outcome else {
        panic!("Terminated bekleniyordu");
    };
    assert_eq!(end_response["reason"], json!("SLA.Dwell"));
    assert_eq!(end_response["node"], json!("self__creditAnalyst"));
    assert_eq!(commit.wfah_entries[0].action, "escalate:self__creditAnalyst:0");
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

/// `Terminated` (SLA ihlali) `Terminal` ile AYNI korumaya tabidir: aksiyon/claim
/// reddedilir, escalation/possible-actions boş döner (2026-07-16 sözleşmesi).
#[tokio::test]
async fn terminated_wfe_is_treated_like_terminal() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());
    let mut wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());
    wfes.status = WfeStatus::Terminated;

    let err = engine
        .apply(&golden(), &wfes, &m, "manager_decide", &json!({"manager_decision": "approve"}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeTerminal));
    assert_eq!(engine.can_claim(&golden(), &wfes, &m).await.unwrap(), ClaimCheck::Terminal);
    assert!(engine.possible_actions(&golden(), &wfes, &m).await.unwrap().is_empty());
    assert_eq!(engine.next_escalation(&golden(), &wfes, Utc::now()).unwrap(), None);

    // wire format kontrolü: serde "terminated" olarak yazar
    assert_eq!(serde_json::to_value(&wfes.status).unwrap(), json!("terminated"));
}

/// Regresyon: deadline geçmiş ama status hâlâ `Active` (sweeper 60s tick'e kadar
/// henüz `terminated`'a taşımadı) — claim/apply bu ARA PENCEREDE de reddedilmeli.
/// Bug: "süresi geçmiş iş claim edilip aksiyon alınabiliyordu, durum terminated
/// olarak bitiyordu" — kök neden, expiry'nin yalnızca 60s sweeper tarafından
/// materialize edilmesi, claim/apply yolunun request-time deadline kontrolü
/// yapmamasıydı (2026-07-16 fix).
#[tokio::test]
async fn expired_but_not_yet_swept_wfe_rejects_claim_and_apply() {
    let org = MockOrg { role_assigned: true };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine { org: &org, exec: &runner };
    let m = manager(Uuid::new_v4());

    // unclaimed, status hâlâ Active, deadline geçmiş → can_claim Expired döner (Ok DEĞİL)
    let mut unclaimed = wfes_at("self__branchManager", None, start_input());
    unclaimed.deadline = Some(unclaimed.created_at - Duration::hours(1));
    assert_eq!(unclaimed.status, WfeStatus::Active);
    assert_eq!(
        engine.can_claim(&golden(), &unclaimed, &m).await.unwrap(),
        ClaimCheck::Expired,
        "deadline geçmiş ama status hâlâ active olan WFE claim edilebilir görünmemeli"
    );

    // zaten claim edilmiş, status hâlâ Active, deadline geçmiş → apply reddedilir
    let mut claimed = wfes_at("self__branchManager", Some(m.user_id), start_input());
    claimed.deadline = Some(claimed.created_at - Duration::hours(1));
    assert_eq!(claimed.status, WfeStatus::Active);
    let err = engine
        .apply(&golden(), &claimed, &m, "manager_decide", &json!({"manager_decision": "approve"}))
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeExpired), "{err}");
    assert!(engine.possible_actions(&golden(), &claimed, &m).await.unwrap().is_empty());
}
