//! §7 pipeline entegrasyon testleri — golden fixture üzerinden uçtan uca.
//! WOR-36 (ilk-match), WOR-39 (WFT formları), WOR-41/45 (trigger retry/catch),
//! WOR-42 (terminal), WOR-46 (timeout), WOR-47 (escalation).
//! Zaman: tokio start_paused — sleep'ler gerçek beklemeden ilerler.

use async_trait::async_trait;
use chrono::{Duration, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::{Wfah, WfahEntry};
use wfe_core::types::wfd_v22::{
    AutoexecDef, AutoexecType, COrgu, CandidateActor, ClaimTimeout, EscalationStep, Wfd,
    WfesEffects, Wft, WftTarget, JoinRule,};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::{ClaimCheck, ClaimTimeoutOutcome, Engine};
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CommitOutcome, ExecEnv, ExecFailure, Wfes,
};

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");

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

// ---- katı org mock: gerçek adapter gibi NIL anchor'ı reddeder ----
//
// Prod'da `OrgAdapter::resolve_c_orgu` → `repo::user_role::resolve_orgu` →
// `orgu::get_orgt_id(anchor)`; anchor yoksa `not found: orgu <id>` döner. Global
// tip seçicisi (`*:[...]`) anchor'a BAKMADAN çözülür (erken dönüş). `MockOrg`
// nil anchor'ı sessizce kabul ettiği için SLA yollarındaki nil-anchor hatası
// testlerden kaçmıştı; bu mock prod davranışını yansıtır.
struct StrictAnchorOrg;

#[async_trait]
impl OrgPort for StrictAnchorOrg {
    async fn resolve_c_orgu(
        &self,
        anchor: Uuid,
        expr: &str,
        _orgtnt: Uuid,
    ) -> Result<Vec<OrgUnit>, EngineError> {
        if !expr.starts_with("*:") && anchor.is_nil() {
            return Err(EngineError::OrgPort(format!("not found: orgu {anchor}")));
        }
        Ok(vec![OrgUnit {
            orgu_id: if anchor.is_nil() { Uuid::new_v4() } else { anchor },
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

fn start_input() -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": 30000, "purpose": "ev tadilatı"}
    })
}

/// self__creditAnalyst node'unda bekleyen, analiste atanmış bir WFES kurar.
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
        claimed_at: assigned.map(|_| created_at),
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
    }
}

// ================================================================ start

#[tokio::test]
async fn start_moves_to_analyst_node_with_real_wfe_id_effects() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let orgu = Uuid::new_v4();
    let actor = clerk(orgu);
    let wfe_id = Uuid::new_v4();

    let new = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            wfe_id,
            None,
        )
        .await
        .unwrap();

    assert_eq!(new.wfe_id, wfe_id);
    assert!(
        matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst")
    );
    // initiated_by = $actor gerçek aktörle çözülmeli
    assert_eq!(
        new.initial_dynctx["initiated_by"]["role"],
        json!("branchClerk")
    );
    assert_eq!(
        new.initial_dynctx["applicant"]["name"],
        json!("Ayşe Yılmaz")
    );
    // M16: WFAH start kaydı gerçek action adını taşır (rezerve "start" değil)
    assert_eq!(new.wfah_entries[0].action, "create_application");
    assert!(new.resolved_c_a.iter().any(|c| c.role == "creditAnalyst"));
}

#[tokio::test]
async fn declared_input_alone_does_not_reach_ctx() {
    // WOR-70: context'e tek yazma yolu wfes_effects'tir. Girdi bildirilmiş ve
    // gönderilmiş olsa bile, onu yazan bir effect yoksa ctx'e GİRMEZ.
    // (Validator böyle bir WFD'yi `unused_action_input` ile reddeder; bu test
    // runtime davranışını yalıtarak doğrular.)
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["start"][0]["wfes_effects"]["set"] = json!({ "initiated_by": "$actor" });
    let wfd = Wfd::from_value(v).unwrap();

    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .expect("input sözleşmesi geçerli — start başarılı olmalı");
    assert!(
        new.initial_dynctx.get("applicant").is_none(),
        "effect yazmadan input ctx'e sızmamalı: {:?}",
        new.initial_dynctx
    );
    assert!(
        new.initial_dynctx.get("credit_info").is_none(),
        "effect yazmadan input ctx'e sızmamalı: {:?}",
        new.initial_dynctx
    );
    assert_eq!(
        new.initial_dynctx["initiated_by"]["role"],
        json!("branchClerk"),
        "effect ile yazılan alan yerinde olmalı"
    );
}

#[tokio::test]
async fn absent_optional_input_nulls_the_field() {
    // WOR-70b: `internal_notes` opsiyonel; gönderilmediğinde manager_decide'ın effect'i
    // onu `null` yazar — escalation'ın yazdığı not KAYBOLUR. Bu, optional'ın required'dan
    // tek farkıdır (required gönderilmek zorunda ve null olamaz). Validator aynı alanı
    // iki yazarın yazdığını `optional_input_nulls_other_writer` uyarısıyla bildirir.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let wfes = wfes_at(
        "self__branchManager",
        Some(m.user_id),
        json!({
            "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
            "credit_info": {"amount_requested": 5000},
            "internal_notes": "SLA aşımı: analist 3 gün içinde işlem yapmadı, müdüre eskalasyon."
        }),
    );

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "reject"}),
            None,
        )
        .await
        .expect("aksiyon uygulanmalı");
    assert_eq!(
        commit.new_dynctx["internal_notes"],
        Value::Null,
        "gönderilmeyen opsiyonel input alanı null'a çevirmeli"
    );
    assert_eq!(commit.new_dynctx["manager_decision"], json!("reject"));
}

#[tokio::test]
async fn start_rejects_undeclared_input_path() {
    // §7.5 simetrisi: start action'ın input tanımında olmayan yol ctx'e sızamaz.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["status"] = json!("approved"); // bildirilmemiş alan — enjeksiyon denemesi

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &input,
            Uuid::new_v4(),
            None,
        )
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
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());
    let input = json!({"applicant": {"name": "Ayşe"}}); // credit_info yok

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &input,
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidInput(m) if m.contains("zorunlu input")),
        "{err}"
    );
}

#[tokio::test]
async fn start_rejects_readonly_field_in_input() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["credit_score"] = json!(999);

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &input,
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn start_with_named_action_selects_matching_rule() {
    // M16: start.action gerçek ad — istemci action adı verirse yalnız o kural aday olur.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            Some("create_application"),
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(new.wfah_entries[0].action, "create_application");
}

#[tokio::test]
async fn start_with_unknown_action_is_not_eligible() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            Some("ghost_action"),
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::StartNotEligible));
}

#[tokio::test]
async fn start_ineligible_actor_is_rejected() {
    let org = MockOrg {
        role_assigned: false,
    }; // rol ataması yok
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::StartNotEligible));
}

// ================================================================ apply — assignment

#[tokio::test]
async fn apply_on_unclaimed_wfe_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 30000}}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotClaimed));
}

#[tokio::test]
async fn apply_by_non_owner_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 30000}}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotOwner));
}

// ================================================================ apply — happy path

#[tokio::test(start_paused = true)]
async fn analyst_approve_within_limit_reaches_terminal_approved() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 30000}}),
            None,
        )
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
    let actions: Vec<&str> = commit
        .wfah_entries
        .iter()
        .map(|e| e.action.as_str())
        .collect();
    assert_eq!(
        actions,
        vec![
            "analyst_approve",
            "trigger:kredi_skoru_getir",
            "trigger:limit_kontrol",
            "trigger:audit_log"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn analyst_approve_over_limit_routes_to_branch_manager() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(650, "C", false); // skor düşük → within_limit false
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 90000}}),
            None,
        )
        .await
        .unwrap();

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "default branch şube müdürüne gitmeli: {:?}",
        commit.outcome
    );
    assert!(commit
        .resolved_c_a
        .iter()
        .any(|c| c.role == "branchManager"));
}

// ================================================================ trigger retry / catch

#[tokio::test(start_paused = true)]
async fn failing_score_fetch_is_retried_then_caught_and_routed() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner {
        rest: RestBehavior::AlwaysFail,
        calc: json!({"within_limit": true}),
        rest_calls: AtomicU32::new(0),
    };
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 30000}}),
            None,
        )
        .await
        .unwrap();

    // ASL: max_attempts=3 retry → toplam 4 çağrı
    assert_eq!(runner.rest_calls.load(Ordering::SeqCst), 4);
    // catch effects staged
    assert_eq!(commit.new_dynctx["score_fetch_failed"], json!(true));
    // limit_kontrol when=false → atlanmış olmalı (within_limit yok)
    assert!(commit.new_dynctx.get("within_limit").is_none());
    // wft ilk condition → şube müdürü
    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
    );
    // handled trigger WFAH'ta işaretli
    let trig = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "trigger:kredi_skoru_getir")
        .unwrap();
    assert_eq!(trig.input.as_ref().unwrap()["handled"], json!(true));
}

#[tokio::test(start_paused = true)]
async fn hanging_autoexec_times_out_and_is_caught() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner {
        rest: RestBehavior::Hang, // 10s timeout'u aşar
        calc: json!({"within_limit": true}),
        rest_calls: AtomicU32::new(0),
    };
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 30000}}),
            None,
        )
        .await
        .unwrap();

    // WFD.Timeout retry listesinde → 4 deneme, sonra catch
    assert_eq!(runner.rest_calls.load(Ordering::SeqCst), 4);
    assert_eq!(commit.new_dynctx["score_fetch_failed"], json!(true));
    let trig = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "trigger:kredi_skoru_getir")
        .unwrap();
    assert_eq!(trig.input.as_ref().unwrap()["error"], json!("WFD.Timeout"));
}

// ================================================================ manager decide

#[tokio::test(start_paused = true)]
async fn manager_reject_takes_default_terminal() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let mut ctx = start_input();
    ctx["score_fetch_failed"] = json!(true);
    let wfes = wfes_at("self__branchManager", Some(m.user_id), ctx);

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "reject"}),
            None,
        )
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
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap();

    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("terminal bekleniyordu");
    };
    assert_eq!(end_response["status"], json!("approved"));
}

#[tokio::test]
async fn undeclared_input_path_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve", "credit_score": 999}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn missing_required_input_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(&golden(), &wfes, &m, "manager_decide", &json!({}), None)
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
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    // 30000 < 1000000 → t_guarded'ın when'i false → t_fallback seçilmeli
    let commit = engine
        .apply(
            &first_match_wfd(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::Terminal { .. }),
        "fallback transition'ın wft'si (terminal) seçilmeliydi: {:?}",
        commit.outcome
    );
}

// ================================================================ NoConditionMatched (M3)

#[tokio::test(start_paused = true)]
async fn conditional_without_default_and_no_match_errors() {
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["transitions"][1]["wft"] = json!({
        "conditions": [{"when": "$action.input.manager_decision == 'never'", "terminal": "terminal_approved"}]
    });
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(
            &wfd,
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NoConditionMatched), "{err}");
}

// ================================================================ claim

#[tokio::test]
async fn claim_checks() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();

    let a = analyst(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    assert_eq!(
        engine.can_claim(&wfd, &wfes, &a, None).await.unwrap(),
        ClaimCheck::Ok
    );

    // yanlış rol → uygun değil
    let c = clerk(Uuid::new_v4());
    assert_eq!(
        engine.can_claim(&wfd, &wfes, &c, None).await.unwrap(),
        ClaimCheck::NotEligible
    );

    // zaten claim edilmiş
    let claimed = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    assert_eq!(
        engine.can_claim(&wfd, &claimed, &a, None).await.unwrap(),
        ClaimCheck::AlreadyClaimed
    );
}

// ================================================================ possible actions

#[tokio::test]
async fn owner_sees_available_actions() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let actions = engine
        .possible_actions(&golden(), &wfes, &m, None)
        .await
        .unwrap();
    assert_eq!(actions, vec!["manager_decide"]);

    // owner olmayan boş liste alır
    let other = manager(Uuid::new_v4());
    let actions = engine
        .possible_actions(&golden(), &wfes, &other, None)
        .await
        .unwrap();
    assert!(actions.is_empty());
}

// ================================================================ escalation (M6)

#[tokio::test]
async fn escalation_fires_after_sla_and_moves_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    // P3D dolmadan due değil
    assert_eq!(
        engine
            .due_escalation(&wfd, &wfes, entered_at + Duration::days(2), None)
            .unwrap(),
        None
    );
    // P3D sonrası due
    let now = entered_at + Duration::days(3) + Duration::seconds(1);
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, now, None).unwrap(),
        Some(0)
    );

    // fire → şube müdürüne taşınır, effects uygulanır (assigned olsa bile çalışır)
    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
    );
    assert!(commit.new_dynctx["internal_notes"]
        .as_str()
        .unwrap()
        .contains("SLA"));
    assert_eq!(
        commit.wfah_entries[0].action,
        "escalate:self__creditAnalyst:0"
    );
    assert_eq!(commit.wfah_entries[0].actor.role, "system");
}

/// Geçmişinde gerçek bir aktör olan WFE — prod durumu. `wfes_at`'in wfah'ı yalnız
/// nil-orgu'lu `system` girdisi taşır; anchor'lı sistem aktörünü sınamak için
/// insan aktör gerekir.
fn wfes_with_human_history(node: &str, assigned: Option<Uuid>, ctx: Value) -> (Wfes, Uuid) {
    let human_orgu = Uuid::new_v4();
    let mut wfes = wfes_at(node, assigned, ctx);
    let wfah = Wfah::empty().push("submitApplication".into(), clerk(human_orgu), None);
    wfes.created_at = wfah.entries()[0].applied_at;
    wfes.claimed_at = assigned.map(|_| wfes.created_at);
    wfes.wfah = wfah;
    (wfes, human_orgu)
}

/// SLA-2: escalation, hedefin c_a'sının YANINDA `wfd.listable` kriterlerini de
/// çözer (`node_candidates`). Golden fixture'ın listable'ı `self`/`parent`
/// çapalıdır — saf sistem aktörünün nil orgu'su ile çözülemez. Prod'da bu, timer
/// süpürücüsünün her turda aynı hatayı vermesine (sonsuz WARN döngüsü) yol açtı:
/// escalation hiç commit edilemediği için vade geçmiş kalıyor.
#[tokio::test]
async fn escalation_resolves_anchored_listable_via_wfah_actor() {
    let org = StrictAnchorOrg;
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let (wfes, human_orgu) = wfes_with_human_history("self__creditAnalyst", None, start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;
    let now = entered_at + Duration::days(3) + Duration::seconds(1);

    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .expect("escalation nil-anchor hatası vermeden çözülmeli");

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
    );
    // Çözüm wfah'taki son insan aktörüne çapalanır — listable `self` buna göre çözülür.
    assert!(commit
        .resolved_c_a
        .iter()
        .any(|c| c.orgu_id == human_orgu));
    // Audit izi DEĞİŞMEZ: marker yine nil-orgu'lu saf `system` aktörüdür.
    assert_eq!(commit.wfah_entries[0].actor.role, "system");
    assert!(commit.wfah_entries[0].actor.orgu_id.is_nil());
}

/// SLA-1 Move yolu aynı çözümü yapar → aynı çapa gerekir.
#[tokio::test]
async fn claim_timeout_move_resolves_anchored_listable_via_wfah_actor() {
    let org = StrictAnchorOrg;
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout("PT1H", Some("self__branchManager"));
    let (wfes, human_orgu) =
        wfes_with_human_history("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let now = wfes.claimed_at.unwrap() + Duration::hours(1) + Duration::seconds(1);

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .expect("claim timeout nil-anchor hatası vermeden çözülmeli")
    {
        ClaimTimeoutOutcome::Move(commit) => {
            assert!(commit
                .resolved_c_a
                .iter()
                .any(|c| c.orgu_id == human_orgu));
            assert_eq!(commit.wfah_entries[0].actor.role, "system");
            assert!(commit.wfah_entries[0].actor.orgu_id.is_nil());
        }
        ClaimTimeoutOutcome::Release(_) => panic!("wft varken Move bekleniyordu"),
    }
}

#[tokio::test]
async fn fired_escalation_step_does_not_refire() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    // adım ateşlenmiş gibi işaretle
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    wfes.wfah = wfes
        .wfah
        .push("escalate:self__creditAnalyst:0".into(), system, None);

    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;
    let now = entered_at + Duration::days(10);
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, now, None).unwrap(),
        None,
        "ateşlenen adım tekrar due olmamalı"
    );
}

// #4 — çok-adımlı aynı-node escalation: her adımın `after`'ı NODE GİRİŞİNDEN ölçülür,
// bir önceki adımın marker'ından değil. Adım 0 (P3D), node'a girişten 3 gün SONRA
// ateşlenmiş olsa bile adım 1 (P5D) yine node girişinden +5 günde due olmalı (+8'de değil).
#[tokio::test]
async fn multi_step_escalation_measures_after_from_node_entry() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };

    // Golden'a creditAnalyst node'una ikinci bir escalation adımı (P5D) ekle.
    let mut wfd = golden();
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P5D".into(),
            wfes_effects: None,
            wft: Some(Wft::Node {
                node: "self__branchManager".into(),
            }),
            terminate: None,
        });

    let t0 = Utc::now();
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    // Kontrollü WFAH: node girişi T0; adım 0 marker'ı T0+3g (gün sonra ateşlendi).
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.wfah = Wfah(vec![
        WfahEntry {
            seq: 1,
            action: "start".into(),
            actor: system.clone(),
            input: None,
            applied_at: t0,
        },
        WfahEntry {
            seq: 2,
            action: "escalate:self__creditAnalyst:0".into(),
            actor: system.clone(),
            input: None,
            applied_at: t0 + Duration::days(3),
        },
    ]);

    // Adım 0 ateşlendi; adım 1 (P5D) node girişinden +4 günde henüz due DEĞİL.
    assert_eq!(
        engine
            .due_escalation(&wfd, &wfes, t0 + Duration::days(4), None)
            .unwrap(),
        None,
        "adım 1 node girişinden +5g'de due olmalı, +4g'de değil",
    );
    // Node girişinden +5 gün + 1sn: adım 1 due (marker'dan ölçülseydi +8g olurdu).
    assert_eq!(
        engine
            .due_escalation(
                &wfd,
                &wfes,
                t0 + Duration::days(5) + Duration::seconds(1),
                None
            )
            .unwrap(),
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
    wfd.start[0].wft = Wft::Node {
        node: "type_branch__branchClerk".into(),
    };

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    assert!(
        matches!(&new.outcome, CommitOutcome::MoveTo { node } if node == "type_branch__branchClerk"),
        "{:?}",
        new.outcome
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
            wft: Some(Wft::Node {
                node: "self__creditAnalyst".into(),
            }),
            terminate: None,
        });

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfes = wfes_at("type_branch__branchClerk", None, start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    assert_eq!(
        engine
            .due_escalation(&wfd, &wfes, entered_at + Duration::hours(12), None)
            .unwrap(),
        None
    );
    let now = entered_at + Duration::days(1) + Duration::seconds(1);
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, now, None).unwrap(),
        Some(0)
    );

    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__creditAnalyst")
    );
}

// ================================================================ SLA-3 deadline (2026-07-16)

#[tokio::test]
async fn deadline_due_fires_terminated_not_error() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
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
async fn start_resolves_deadline_and_allows_exceeding_wfd_timeout() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden(); // timeout: P30D
    let actor = clerk(Uuid::new_v4());

    // deadline > wfd.timeout → artık serbest, çağıran WFD tavanını aşabilir
    let before = Utc::now();
    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            Some("P40D"),
        )
        .await
        .unwrap();
    let deadline = new
        .deadline
        .expect("deadline verildiğinde resolve edilmeli");
    assert!(deadline >= before + Duration::days(40) && deadline <= Utc::now() + Duration::days(40));

    // deadline ≤ timeout → kabul, mutlak deadline start anından itibaren çözülür
    let before = Utc::now();
    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            Some("P10D"),
        )
        .await
        .unwrap();
    let deadline = new
        .deadline
        .expect("deadline verildiğinde resolve edilmeli");
    assert!(deadline >= before + Duration::days(10) && deadline <= Utc::now() + Duration::days(10));

    // deadline verilmedi, wfd.timeout var → wfd.timeout kullanılır
    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    let deadline = new
        .deadline
        .expect("wfd.timeout varken deadline resolve edilmeli");
    assert!(deadline >= before + Duration::days(30) && deadline <= Utc::now() + Duration::days(30));
}

#[tokio::test]
async fn start_without_deadline_or_timeout_leaves_deadline_null() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = golden();
    wfd.timeout = None;
    let actor = clerk(Uuid::new_v4());

    let new = engine
        .start(
            &wfd,
            &actor,
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();
    assert!(new.deadline.is_none());
}

// ================================================================ SLA-1 claim timeout (2026-07-16)

/// self__creditAnalyst'e claim_timeout ekleyen golden varyantı.
fn golden_with_claim_timeout(after: &str, wft: Option<&str>) -> Wfd {
    let mut wfd = golden();
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .claim_timeout = Some(ClaimTimeout {
        after: after.into(),
        wfes_effects: None,
        wft: wft.map(String::from),
        collapses_parallel: false,
    });
    wfd
}

#[tokio::test]
async fn claim_timeout_due_without_wft_releases_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout("PT2H", None);
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);

    assert!(!engine
        .claim_timeout_due(&wfd, &wfes, claimed_at + Duration::hours(1), None)
        .unwrap());
    let now = claimed_at + Duration::hours(2) + Duration::seconds(1);
    assert!(engine.claim_timeout_due(&wfd, &wfes, now, None).unwrap());

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Release(release) => {
            assert_eq!(
                release.wfah_entry.action,
                "claim_timeout:self__creditAnalyst"
            );
            assert_eq!(release.wfah_entry.actor.role, "system");
            // wfes_effects yok → ctx satırı yazılmaz
            assert!(release.new_dynctx.is_none());
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

#[tokio::test]
async fn claim_timeout_due_with_wft_moves_like_escalation() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout("PT1H", Some("self__branchManager"));
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);
    let now = claimed_at + Duration::hours(1) + Duration::seconds(1);

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Move(commit) => {
            assert!(
                matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
            );
            assert_eq!(
                commit.wfah_entries[0].action,
                "claim_timeout:self__creditAnalyst"
            );
        }
        ClaimTimeoutOutcome::Release(_) => panic!("wft varken Move bekleniyordu"),
    }
}

/// WOR-56/SLA-1 (2026-08-03): `collapses_parallel` işaretli olsa bile WFE paralel
/// modda DEĞİLSE bayrak yok sayılır — normal `{node}` devri uygulanır. Aksi halde
/// `resolve_wft` collapse'ı Single modda reddeder ve WFE zaman aşımında kilitlenirdi
/// (aynı node kol içinden de kol dışından da erişilebilir).
#[tokio::test]
async fn claim_timeout_collapse_flag_ignored_outside_parallel() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = golden_with_claim_timeout("PT1H", Some("self__branchManager"));
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .claim_timeout
        .as_mut()
        .unwrap()
        .collapses_parallel = true;
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);
    let now = claimed_at + Duration::hours(1) + Duration::seconds(1);

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Move(commit) => {
            assert!(
                matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
                "paralel dışı: collapse DEĞİL düz devir bekleniyordu — {:?}",
                commit.outcome
            );
            // audit'te `collapse` anahtarı yazılmaz (yalnız gerçek collapse'ta).
            let input = commit.wfah_entries[0].input.as_ref().unwrap();
            assert!(input.get("collapse").is_none());
        }
        ClaimTimeoutOutcome::Release(_) => panic!("wft varken Move bekleniyordu"),
    }
}

// ---- 2026-07-28: SLA-1 wfes_effects (opsiyonel DynCtx yazımı) ----

/// `golden_with_claim_timeout` + `wfes_effects` — SLA-1 dolduğunda ctx'e yazar.
fn golden_with_claim_timeout_effects(after: &str, wft: Option<&str>) -> Wfd {
    let mut wfd = golden_with_claim_timeout(after, wft);
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .claim_timeout
        .as_mut()
        .unwrap()
        .wfes_effects = Some(WfesEffects {
        set: BTreeMap::from([
            (
                "internal_notes".to_string(),
                json!("Claim süresi doldu, iş havuza döndü."),
            ),
            ("analyst_approved_at".to_string(), json!("$timestamp")),
        ]),
    });
    wfd
}

#[tokio::test]
async fn claim_timeout_release_applies_wfes_effects() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout_effects("PT2H", None);
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);
    let now = claimed_at + Duration::hours(2) + Duration::seconds(1);

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Release(release) => {
            let ctx = release
                .new_dynctx
                .expect("wfes_effects varken yeni ctx bekleniyordu");
            assert_eq!(
                ctx["internal_notes"],
                json!("Claim süresi doldu, iş havuza döndü.")
            );
            assert_eq!(ctx["analyst_approved_at"], json!(now.to_rfc3339()));
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

#[tokio::test]
async fn claim_timeout_move_applies_wfes_effects_before_wft() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout_effects("PT1H", Some("self__branchManager"));
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    let claimed_at = wfes.created_at;
    wfes.claimed_at = Some(claimed_at);
    let now = claimed_at + Duration::hours(1) + Duration::seconds(1);

    match engine
        .fire_claim_timeout(&wfd, &wfes, now, None)
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Move(commit) => {
            assert!(
                matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager")
            );
            assert_eq!(
                commit.new_dynctx["internal_notes"],
                json!("Claim süresi doldu, iş havuza döndü.")
            );
        }
        ClaimTimeoutOutcome::Release(_) => panic!("wft varken Move bekleniyordu"),
    }
}

#[tokio::test]
async fn claim_timeout_not_due_without_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_claim_timeout("PT1H", None);
    // hiç claim edilmemiş (claimed_at None) — asla due olmaz
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    assert!(!engine
        .claim_timeout_due(&wfd, &wfes, wfes.created_at + Duration::days(1), None)
        .unwrap());
}

// ============================================ SLA-2 akışı BİTİREMEZ (2026-07-28)

/// `terminate` kaldırıldı: hedefsiz bir escalation adımı artık akışı sonlandırmaz,
/// hata verir. Akışı zaman aşımıyla bitiren TEK kural SLA-3 (root `timeout`) —
/// bkz. `deadline_due_fires_terminated_not_error`.
#[tokio::test]
async fn escalation_without_wft_errors_instead_of_terminating() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = golden();
    {
        let esc = &mut wfd.nodes.get_mut("self__creditAnalyst").unwrap().escalation[0];
        esc.wft = None;
        esc.terminate = Some(true); // eski alan; runtime artık dikkate almaz
    }
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    let now = wfes.created_at + Duration::days(3) + Duration::seconds(1);

    let err = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .expect_err("hedefsiz escalation hata vermeli");
    assert!(matches!(err, EngineError::InvalidWfd(_)), "err: {err:?}");
}

/// Hedefi olan adım normal node devri yapar — `Terminated` ASLA üretmez.
#[tokio::test]
async fn escalation_with_node_target_moves_and_never_terminates() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    let now = wfes.created_at + Duration::days(3) + Duration::seconds(1);

    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "outcome: {:?}",
        commit.outcome
    );
    assert_eq!(
        commit.wfah_entries[0].action,
        "escalate:self__creditAnalyst:0"
    );
}

// ================================================================ terminal WFE korunur

#[tokio::test]
async fn terminal_wfe_rejects_actions() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let mut wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());
    wfes.status = WfeStatus::Terminal;

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeTerminal));
    assert_eq!(
        engine.can_claim(&golden(), &wfes, &m, None).await.unwrap(),
        ClaimCheck::Terminal
    );
}

/// `Terminated` (SLA ihlali) `Terminal` ile AYNI korumaya tabidir: aksiyon/claim
/// reddedilir, escalation/possible-actions boş döner (2026-07-16 sözleşmesi).
#[tokio::test]
async fn terminated_wfe_is_treated_like_terminal() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());
    let mut wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());
    wfes.status = WfeStatus::Terminated;

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeTerminal));
    assert_eq!(
        engine.can_claim(&golden(), &wfes, &m, None).await.unwrap(),
        ClaimCheck::Terminal
    );
    assert!(engine
        .possible_actions(&golden(), &wfes, &m, None)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        engine
            .next_escalation(&golden(), &wfes, Utc::now(), None)
            .unwrap(),
        None
    );

    // wire format kontrolü: serde "terminated" olarak yazar
    assert_eq!(
        serde_json::to_value(&wfes.status).unwrap(),
        json!("terminated")
    );
}

/// Regresyon: deadline geçmiş ama status hâlâ `Active` (sweeper 60s tick'e kadar
/// henüz `terminated`'a taşımadı) — claim/apply bu ARA PENCEREDE de reddedilmeli.
/// Bug: "süresi geçmiş iş claim edilip aksiyon alınabiliyordu, durum terminated
/// olarak bitiyordu" — kök neden, expiry'nin yalnızca 60s sweeper tarafından
/// materialize edilmesi, claim/apply yolunun request-time deadline kontrolü
/// yapmamasıydı (2026-07-16 fix).
#[tokio::test]
async fn expired_but_not_yet_swept_wfe_rejects_claim_and_apply() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let m = manager(Uuid::new_v4());

    // unclaimed, status hâlâ Active, deadline geçmiş → can_claim Expired döner (Ok DEĞİL)
    let mut unclaimed = wfes_at("self__branchManager", None, start_input());
    unclaimed.deadline = Some(unclaimed.created_at - Duration::hours(1));
    assert_eq!(unclaimed.status, WfeStatus::Active);
    assert_eq!(
        engine
            .can_claim(&golden(), &unclaimed, &m, None)
            .await
            .unwrap(),
        ClaimCheck::Expired,
        "deadline geçmiş ama status hâlâ active olan WFE claim edilebilir görünmemeli"
    );

    // zaten claim edilmiş, status hâlâ Active, deadline geçmiş → apply reddedilir
    let mut claimed = wfes_at("self__branchManager", Some(m.user_id), start_input());
    claimed.deadline = Some(claimed.created_at - Duration::hours(1));
    assert_eq!(claimed.status, WfeStatus::Active);
    let err = engine
        .apply(
            &golden(),
            &claimed,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve"}),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::WfeExpired), "{err}");
    assert!(engine
        .possible_actions(&golden(), &claimed, &m, None)
        .await
        .unwrap()
        .is_empty());
}

// ================================================================ WOR-31 fork/join (paralel)

const PARALLEL_FIXTURE: &str = include_str!("fixtures/paralel-onay.json");

fn paralel() -> Wfd {
    Wfd::from_json(PARALLEL_FIXTURE).unwrap()
}

fn parallel_ctx() -> Value {
    json!({"request": {"title": "Sunucu alımı", "amount": 150000}})
}

fn actor_with_role(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

fn branch(node: &str, status: BranchStatus, claimed_by: Option<Uuid>) -> BranchState {
    let now = Utc::now();
    BranchState {
        entry_node: node.into(),
        branch_node: node.into(),
        status,
        claimed_by,
        claimed_at: claimed_by.map(|_| now),
        entered_at: now,
    }
}

/// Fork SONRASI paralel modda bir WFES kurar: current_node NULL, join persist.
fn parallel_wfes(branches: Vec<BranchState>, join: WftTarget, ctx: Value) -> Wfes {
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
        current_node: None,
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches,
        join_target: Some(join),
        // WOR-72: bu yardımcı AND-join kurar; quorum/expr testleri kendi kuralını verir.
        join_rule: JoinRule::All,
    }
}

fn join_node() -> WftTarget {
    WftTarget::Node {
        node: "self__resultCoordinator".into(),
    }
}

fn wfah_actions(commit: &wfe_core::v22::ports::TransitionCommit) -> Vec<&str> {
    commit
        .wfah_entries
        .iter()
        .map(|e| e.action.as_str())
        .collect()
}

#[tokio::test]
async fn start_review_forks_into_three_branches() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let coord = actor_with_role("coordinator");
    let wfes = wfes_at("self__coordinator", Some(coord.user_id), parallel_ctx());

    let commit = engine
        .apply(&paralel(), &wfes, &coord, "start_review", &json!({}), None)
        .await
        .unwrap();

    let CommitOutcome::ForkTo { branches, join, .. } = &commit.outcome else {
        panic!("ForkTo bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(
        branches,
        &[
            "self__financeApprover",
            "self__legalApprover",
            "self__hrApprover"
        ]
    );
    assert_eq!(join, &join_node());
    // aday cache'i üç kolun rollerinin birleşimi
    for role in ["financeApprover", "legalApprover", "hrApprover"] {
        assert!(
            commit.resolved_c_a.iter().any(|c| c.role == role),
            "{role} eksik"
        );
    }
    // `_fork` marker'ı engine tarafından staged (system aktörle)
    assert_eq!(wfah_actions(&commit), vec!["start_review", "_fork"]);
    let fork = commit.wfah_entries.last().unwrap();
    assert_eq!(fork.actor.role, "system");
    assert_eq!(
        fork.input.as_ref().unwrap()["branches"][1],
        json!("self__legalApprover")
    );
    assert_eq!(
        fork.input.as_ref().unwrap()["join"],
        json!({"node": "self__resultCoordinator"})
    );
}

#[tokio::test]
async fn single_mode_node_hint_must_match_current_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let coord = actor_with_role("coordinator");
    let wfes = wfes_at("self__coordinator", Some(coord.user_id), parallel_ctx());

    let err = engine
        .apply(
            &paralel(),
            &wfes,
            &coord,
            "start_review",
            &json!({}),
            Some("self__hrApprover"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn branch_approve_arrives_without_occupying_join() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &paralel(),
            &wfes,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap();

    assert!(
        matches!(&commit.outcome, CommitOutcome::BranchArrived { from_node, .. } if from_node == "self__financeApprover"),
        "{:?}",
        commit.outcome
    );
    // kol transition effect'i staged
    assert!(commit.new_dynctx["finans_onay_zamani"].is_string());
    assert_eq!(wfah_actions(&commit), vec!["approve", "_branch_arrived"]);
    assert_eq!(
        commit.wfah_entries[1].input.as_ref().unwrap()["node"],
        json!("self__financeApprover")
    );
    // varış WFE'yi taşımaz — aday cache boş kalır (kol havuzu T3'te branch satırından)
    assert!(commit.resolved_c_a.is_empty());
}

#[tokio::test]
async fn ambiguous_action_without_node_hint_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    // `approve` üç kolun da transition'ıyla eşleşir — node ipucu yoksa belirsiz
    let err = engine
        .apply(&paralel(), &wfes, &fin, "approve", &json!({}), None)
        .await
        .unwrap_err();
    match err {
        EngineError::AmbiguousAction { action, candidates } => {
            assert_eq!(action, "approve");
            assert_eq!(candidates.len(), 3);
            assert!(candidates.contains(&"self__legalApprover".to_string()));
        }
        other => panic!("AmbiguousAction bekleniyordu: {other}"),
    }

    // geçersiz ipucu: aktif kol değil
    let err = engine
        .apply(
            &paralel(),
            &wfes,
            &fin,
            "approve",
            &json!({}),
            Some("self__coordinator"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

#[tokio::test]
async fn parallel_apply_enforces_branch_claim_ownership() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");

    // kol claim edilmemiş → NotClaimed
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let err = engine
        .apply(
            &paralel(),
            &wfes,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotClaimed), "{err}");

    // başka kullanıcı claim etmiş → NotOwner
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(Uuid::new_v4()),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let err = engine
        .apply(
            &paralel(),
            &wfes,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::NotOwner), "{err}");
}

#[tokio::test]
async fn last_branch_arrival_completes_join_to_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let hr = actor_with_role("hrApprover");
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Arrived, None),
            branch("self__legalApprover", BranchStatus::Arrived, None),
            branch("self__hrApprover", BranchStatus::Active, Some(hr.user_id)),
        ],
        join_node(),
        parallel_ctx(),
    );

    // tek aktif kol kaldığından node ipucu GEREKMEZ (belirsizlik yok)
    let commit = engine
        .apply(&paralel(), &wfes, &hr, "approve", &json!({}), None)
        .await
        .unwrap();

    let CommitOutcome::JoinComplete { from_node, next, .. } = &commit.outcome else {
        panic!("JoinComplete bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(from_node, "self__hrApprover");
    assert!(
        matches!(next.as_ref(), CommitOutcome::MoveTo { node } if node == "self__resultCoordinator"),
        "{next:?}"
    );
    // join node'un adayları promotion için resolve edilir
    assert!(commit
        .resolved_c_a
        .iter()
        .any(|c| c.role == "resultCoordinator"));
    // engine `_branch_arrived` staged eder; `_join` ADAPTER'ın işidir (T3)
    assert_eq!(wfah_actions(&commit), vec!["approve", "_branch_arrived"]);
}

#[tokio::test]
async fn last_branch_arrival_completes_join_to_terminal() {
    // join hedefi terminal olan varyant: kol wft'leri de aynı terminale çözülür,
    // son varışta JoinComplete{next: Terminal} üretilmeli.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["transitions"][0]["wft"]["parallel"]["join"] = json!({"terminal": "terminal_approved"});
    for idx in [1, 3, 5] {
        // t_finance_approve / t_legal_approve / t_hr_approve
        v["transitions"][idx]["wft"] = json!({"terminal": "terminal_approved"});
    }
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let hr = actor_with_role("hrApprover");
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Arrived, None),
            branch("self__legalApprover", BranchStatus::Arrived, None),
            branch("self__hrApprover", BranchStatus::Active, Some(hr.user_id)),
        ],
        WftTarget::Terminal {
            terminal: "terminal_approved".into(),
        },
        parallel_ctx(),
    );

    let commit = engine
        .apply(&wfd, &wfes, &hr, "approve", &json!({}), None)
        .await
        .unwrap();

    let CommitOutcome::JoinComplete { from_node, next, .. } = &commit.outcome else {
        panic!("JoinComplete bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(from_node, "self__hrApprover");
    let CommitOutcome::Terminal { end_response } = next.as_ref() else {
        panic!("Terminal next bekleniyordu: {next:?}");
    };
    assert_eq!(end_response["status"], json!("approved"));
    assert_eq!(end_response["request_title"], json!("Sunucu alımı"));
    assert_eq!(wfah_actions(&commit), vec!["approve", "_branch_arrived"]);
}

#[tokio::test]
async fn branch_reject_ends_wfe_and_cancels_active_siblings() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let legal = actor_with_role("legalApprover");
    // finance hâlâ aktif, hr çoktan vardı — yalnız AKTİF sibling iptal edilir
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch(
                "self__legalApprover",
                BranchStatus::Active,
                Some(legal.user_id),
            ),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &paralel(),
            &wfes,
            &legal,
            "reject",
            &json!({}),
            Some("self__legalApprover"),
        )
        .await
        .unwrap();

    let CommitOutcome::Terminal { end_response } = &commit.outcome else {
        panic!("Terminal bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(end_response["status"], json!("rejected"));
    assert_eq!(end_response["request_title"], json!("Sunucu alımı"));
    // aktif sibling (finance) iptal marker'ı; hr arrived — WOR-60: iptal DEĞİL,
    // superseded marker'ı alır (kol satırı `arrived` kalır).
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "reject",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let cancel = &commit.wfah_entries[2];
    assert_eq!(
        cancel.input.as_ref().unwrap()["node"],
        json!("self__financeApprover")
    );
    assert_eq!(
        cancel.input.as_ref().unwrap()["reason"],
        json!("sibling_terminal")
    );
    assert_eq!(cancel.actor.role, "system");
    let superseded = &commit.wfah_entries[3];
    assert_eq!(
        superseded.input.as_ref().unwrap()["node"],
        json!("self__hrApprover")
    );
    assert_eq!(
        superseded.input.as_ref().unwrap()["reason"],
        json!("sibling_terminal")
    );
}

#[tokio::test]
async fn branch_collapse_to_node_ends_parallel_and_moves_wfe() {
    // WOR-56: kol collapse aksiyonu bir NODE hedefine (fork-initiator = restart).
    // Paralel mod biter, WFE o node'a geçer, AKTİF kardeşler iptal marker'ı alır.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    // finance "reject" transition'ını collapse-to-node yap (hedef: self__coordinator).
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["action"] == json!("reject") && t["from"] == json!("self__financeApprover") {
            t["wft"] = json!({"collapse": {"node": "self__coordinator"}});
        }
    }
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    // finance acting; legal aktif (iptal edilecek); hr çoktan vardı (iptal EDİLMEZ).
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap();

    let CommitOutcome::CollapseTo { from_node, node } = &commit.outcome else {
        panic!("CollapseTo bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(from_node, "self__financeApprover");
    assert_eq!(node, "self__coordinator");
    // hedef node'un adayları promotion için resolve edilir
    assert!(commit.resolved_c_a.iter().any(|c| c.role == "coordinator"));
    // aktif sibling (legal) iptal marker'ı; hr arrived → superseded (WOR-60);
    // acting kol (finance) hiç marker almaz.
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "reject",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let cancel = &commit.wfah_entries[2];
    assert_eq!(
        cancel.input.as_ref().unwrap()["node"],
        json!("self__legalApprover")
    );
    assert_eq!(cancel.input.as_ref().unwrap()["reason"], json!("collapsed"));
    assert_eq!(cancel.actor.role, "system");
    // WOR-59: claim'siz kolda alanlar açıkça null (alan HER ZAMAN var)
    assert!(cancel.input.as_ref().unwrap()["claimed_by"].is_null());
    assert!(cancel.input.as_ref().unwrap()["claimed_at"].is_null());
}

#[tokio::test]
async fn collapse_marker_carries_dropped_claim_owner() {
    // WOR-59: iptal edilen kolun claim'i adapter'da düşürülür; sahibinin ve
    // claimed_at'in TEK kaydı `_branch_cancelled` marker'ıdır.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["action"] == json!("reject") && t["from"] == json!("self__financeApprover") {
            t["wft"] = json!({"collapse": {"node": "self__coordinator"}});
        }
    }
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let legal_owner = Uuid::new_v4();
    let legal = branch(
        "self__legalApprover",
        BranchStatus::Active,
        Some(legal_owner),
    );
    let legal_claimed_at = legal.claimed_at.unwrap();
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            legal,
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap();

    let cancel = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "_branch_cancelled")
        .expect("_branch_cancelled marker");
    let input = cancel.input.as_ref().unwrap();
    assert_eq!(input["node"], json!("self__legalApprover"));
    assert_eq!(input["claimed_by"], json!(legal_owner));
    assert_eq!(input["claimed_at"], json!(legal_claimed_at));
}

#[tokio::test]
async fn collapse_summary_marker_describes_whole_event() {
    // WOR-61: collapse'ın tamamı tek `_collapse` kaydından okunabilmeli —
    // tetikleyen kol/aksiyon, hedef, iptal edilen ve geçersizleşen kollar.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["action"] == json!("reject") && t["from"] == json!("self__financeApprover") {
            t["wft"] = json!({"collapse": {"node": "self__coordinator"}});
        }
    }
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap();

    // manşet DETAY marker'larından önce gelir, detaylar KALIR
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "reject",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let summary = &commit.wfah_entries[1];
    assert_eq!(summary.actor.role, "system");
    let input = summary.input.as_ref().unwrap();
    assert_eq!(input["trigger_branch"], json!("self__financeApprover"));
    assert_eq!(input["trigger_action"], json!("reject"));
    assert_eq!(input["trigger_actor"]["user_id"], json!(fin.user_id));
    assert_eq!(input["kind"], json!("collapse_to"));
    assert_eq!(input["reason"], json!("collapsed"));
    assert_eq!(input["target"], json!("self__coordinator"));
    assert_eq!(input["cancelled"], json!(["self__legalApprover"]));
    assert_eq!(input["superseded"], json!(["self__hrApprover"]));

    // WOR-63: kol marker'ları da tetikleyici bağlamı taşır; `reason` DEĞİŞMEZ.
    for detail in &commit.wfah_entries[2..] {
        let d = detail.input.as_ref().unwrap();
        assert_eq!(d["reason"], json!("collapsed"), "{}", detail.action);
        assert_eq!(
            d["trigger_node"],
            json!("self__financeApprover"),
            "{}",
            detail.action
        );
        assert_eq!(d["trigger_action"], json!("reject"), "{}", detail.action);
        assert_eq!(
            d["trigger_actor"]["user_id"],
            json!(fin.user_id),
            "{}",
            detail.action
        );
        assert_eq!(
            d["trigger_actor"]["role"],
            json!("financeApprover"),
            "{}",
            detail.action
        );
    }
}

/// 2026-07-28 değişmezi: kol SLA-2'si kardeş kolları DÜŞÜRMEZ. `wft` yalnız `{node}`
/// olabildiği için collapse/terminate yolları kapalı — kol yalnız kendi hedefine
/// hareket eder, kardeşler aktif kalır. (WOR-63'ün kol-kapsamlı sistem-collapse
/// senaryosu bu yüzden artık mümkün değil; sistem tetikli collapse'ın TEK yolu SLA-3
/// deadline'ıdır — bkz. `collapse_summary_on_terminal_path_has_null_target`.)
#[tokio::test]
async fn branch_escalation_does_not_touch_sibling_branches() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P1D".into(),
            wfes_effects: None,
            wft: Some(Wft::Node {
                node: "self__coordinator".into(),
            }),
            terminate: None,
        });
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch("self__legalApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let now = wfes.branches[0].entered_at + Duration::days(1) + Duration::seconds(1);

    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, Some("self__financeApprover"))
        .await
        .unwrap();

    assert!(
        matches!(
            &commit.outcome,
            CommitOutcome::BranchMoveTo { from_node, node }
                if from_node == "self__financeApprover" && node == "self__coordinator"
        ),
        "outcome: {:?}",
        commit.outcome
    );
    // Yalnız escalation marker'ı — hiçbir iptal/collapse marker'ı YOK.
    assert_eq!(
        wfah_actions(&commit),
        vec!["escalate:self__financeApprover:0"]
    );
}

#[tokio::test]
async fn collapse_summary_on_terminal_path_has_null_target() {
    // WOR-61: terminal yollarında akış bir node'a gitmez → `target` null,
    // `kind` outcome'u ayırt eder. Sistem yolunda tetikleyici system aktördür.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let now = Utc::now();
    wfes.deadline = Some(now - Duration::hours(1));

    let commit = engine.fire_deadline_timeout(&wfes, now);
    let summary = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "_collapse")
        .expect("_collapse marker");
    let input = summary.input.as_ref().unwrap();
    assert!(
        input["trigger_branch"].is_null(),
        "SLA-3 tek bir koldan tetiklenmez"
    );
    assert_eq!(input["trigger_action"], json!("timeout:deadline"));
    assert_eq!(input["trigger_actor"]["role"], json!("system"));
    assert_eq!(input["kind"], json!("terminated"));
    assert!(input["target"].is_null());
    assert_eq!(input["cancelled"], json!(["self__financeApprover"]));
    assert_eq!(input["superseded"], json!(["self__hrApprover"]));
}

#[tokio::test]
async fn collapse_outside_parallel_is_rejected() {
    // WOR-56: collapse yalnız kol bağlamında geçerli — tekil modda hata.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    // coordinator'ın fork transition'ını collapse'a çevir (tekil modda uygulanır).
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["from"] == json!("self__coordinator") {
            t["action"] = json!("collapse_here");
            t["wft"] = json!({"collapse": {"node": "self__requester"}});
        }
    }
    v["actions"]["collapse_here"] =
        json!({"label": "X", "input": {"required": [], "optional": []}});
    // validator collapse'ı reddetmesin diye şema kontrolünü atlayıp doğrudan runtime'ı
    // sınıyoruz — Wfd::from_value validator çalıştırmaz (yalnız parse).
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let coord = actor_with_role("coordinator");
    let wfes = wfes_at("self__coordinator", Some(coord.user_id), parallel_ctx());

    let err = engine
        .apply(&wfd, &wfes, &coord, "collapse_here", &json!({}), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidWfd(m) if m.contains("collapse")),
        "collapse tekil modda InvalidWfd vermeli: {err:?}"
    );
}

fn paralel_with_delegate_step() -> Wfd {
    // finance koluna kol-içi ara node ekler: delegate → self__financeSenior,
    // oradan approve → join. Kol hareketi (BranchMoveTo) testi için.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["nodes"]["self__financeSenior"] = json!({
        "label": "Finans Kıdemli",
        "description": "Kol-içi ara durak (test).",
        "c_a": {"c_orgu": "self", "c_r": ["financeSenior"]}
    });
    v["actions"]["delegate"] =
        json!({"label": "Devret", "input": {"required": [], "optional": []}});
    let ts = v["transitions"].as_array_mut().unwrap();
    ts.push(json!({
        "id": "t_finance_delegate",
        "from": "self__financeApprover",
        "action": "delegate",
        "wft": {"node": "self__financeSenior"}
    }));
    ts.push(json!({
        "id": "t_senior_approve",
        "from": "self__financeSenior",
        "action": "approve",
        "wft": {"node": "self__resultCoordinator"}
    }));
    Wfd::from_value(v).unwrap()
}

#[tokio::test]
async fn branch_moves_to_normal_node_and_stays_parallel() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    // `delegate` yalnız finance kolunda tanımlı → ipucu gerekmez
    let commit = engine
        .apply(
            &paralel_with_delegate_step(),
            &wfes,
            &fin,
            "delegate",
            &json!({}),
            None,
        )
        .await
        .unwrap();

    assert!(
        matches!(
            &commit.outcome,
            CommitOutcome::BranchMoveTo { from_node, node }
                if from_node == "self__financeApprover" && node == "self__financeSenior"
        ),
        "{:?}",
        commit.outcome
    );
    // yeni kol node'unun adayları resolve edilir
    assert!(commit
        .resolved_c_a
        .iter()
        .any(|c| c.role == "financeSenior"));
    // kol hareketi paralel marker üretmez
    assert_eq!(wfah_actions(&commit), vec!["delegate"]);
}

#[tokio::test]
async fn nested_parallel_at_runtime_is_rejected() {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    // t_finance_approve'un wft'ini parallel yap (validator dışı, runtime koruması)
    v["transitions"][1]["wft"] = json!({
        "parallel": {
            "branches": ["self__legalApprover", "self__hrApprover"],
            "join": {"node": "self__resultCoordinator"}
        }
    });
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let fin = actor_with_role("financeApprover");
    let wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(fin.user_id),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    let err = engine
        .apply(
            &wfd,
            &wfes,
            &fin,
            "approve",
            &json!({}),
            Some("self__financeApprover"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidWfd(m) if m.contains("nested")),
        "{err}"
    );
}

#[tokio::test]
async fn start_wft_parallel_is_rejected_at_runtime() {
    // Validator start'ta parallel'i reddeder; runtime koruması bağımsız çalışmalı.
    let mut wfd = paralel();
    wfd.start[0].wft = wfd.transitions[0].wft.clone(); // t_fork'un parallel wft'i

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let req = actor_with_role("requester");

    let err = engine
        .start(
            &wfd,
            &req,
            Uuid::nil(),
            None,
            &json!({"request": {"title": "X", "amount": 1}}),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidWfd(m) if m.contains("start")),
        "{err}"
    );
}

#[tokio::test]
async fn branch_claim_timeout_measured_from_branch_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .claim_timeout = Some(ClaimTimeout {
        after: "PT2H".into(),
        wfes_effects: None,
        wft: None,
        collapses_parallel: false,
    });

    let fin = Uuid::new_v4();
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, Some(fin)),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let claimed_at = wfes.branches[0].claimed_at.unwrap();

    // kol sayacı kol claimed_at'ından ölçülür
    assert!(!engine
        .claim_timeout_due(
            &wfd,
            &wfes,
            claimed_at + Duration::hours(1),
            Some("self__financeApprover")
        )
        .unwrap());
    let now = claimed_at + Duration::hours(2) + Duration::seconds(1);
    assert!(engine
        .claim_timeout_due(&wfd, &wfes, now, Some("self__financeApprover"))
        .unwrap());
    // claim'siz kol asla due değil
    assert!(!engine
        .claim_timeout_due(&wfd, &wfes, now, Some("self__legalApprover"))
        .unwrap());
    // wfe-seviyesi sayaç paralel modda yok (claimed_at NULL)
    assert!(!engine.claim_timeout_due(&wfd, &wfes, now, None).unwrap());

    // wft'siz kol → Release (kol claim'inin sıfırlanması T3'te kol-farkında persist edilir)
    match engine
        .fire_claim_timeout(&wfd, &wfes, now, Some("self__financeApprover"))
        .await
        .unwrap()
    {
        ClaimTimeoutOutcome::Release(release) => {
            assert_eq!(
                release.wfah_entry.action,
                "claim_timeout:self__financeApprover"
            );
            assert_eq!(release.wfah_entry.actor.role, "system");
            assert!(release.new_dynctx.is_none());
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

#[tokio::test]
async fn branch_escalation_fires_from_branch_entered_at() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P1D".into(),
            wfes_effects: None,
            wft: Some(Wft::Node {
                node: "self__resultCoordinator".into(),
            }),
            terminate: None,
        });

    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let entered_at = wfes.branches[0].entered_at;

    // dwell KOL girişinden ölçülür
    assert_eq!(
        engine
            .due_escalation(
                &wfd,
                &wfes,
                entered_at + Duration::hours(12),
                Some("self__financeApprover")
            )
            .unwrap(),
        None
    );
    let now = entered_at + Duration::days(1) + Duration::seconds(1);
    assert_eq!(
        engine
            .due_escalation(&wfd, &wfes, now, Some("self__financeApprover"))
            .unwrap(),
        Some(0)
    );
    // wfe-seviyesi görünüm paralel modda dwell izlemez (current_node NULL)
    assert_eq!(engine.due_escalation(&wfd, &wfes, now, None).unwrap(), None);

    // escalation wft'i join'i hedefliyor → kol VARIŞ sayılır (2 aktif kol kaldı)
    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, Some("self__financeApprover"))
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::BranchArrived { from_node, .. } if from_node == "self__financeApprover"),
        "{:?}",
        commit.outcome
    );
    assert_eq!(
        wfah_actions(&commit),
        vec!["escalate:self__financeApprover:0", "_branch_arrived"]
    );
}

#[tokio::test]
async fn deadline_in_parallel_mode_cancels_all_active_branches() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let mut wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let now = Utc::now();
    wfes.deadline = Some(now - Duration::hours(1));
    assert!(engine.deadline_due(&wfes, now));

    let commit = engine.fire_deadline_timeout(&wfes, now);
    let CommitOutcome::Terminated { end_response } = &commit.outcome else {
        panic!("Terminated bekleniyordu");
    };
    assert_eq!(end_response["reason"], json!("SLA.Deadline"));
    // her AKTİF kol için `_branch_cancelled`, arrived hr için `_branch_superseded`
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "timeout:deadline",
            "_collapse",
            "_branch_cancelled",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let nodes: Vec<&Value> = commit.wfah_entries[2..]
        .iter()
        .map(|e| &e.input.as_ref().unwrap()["node"])
        .collect();
    assert_eq!(
        nodes,
        vec![
            &json!("self__financeApprover"),
            &json!("self__legalApprover"),
            &json!("self__hrApprover")
        ]
    );
    assert_eq!(
        commit.wfah_entries[2].input.as_ref().unwrap()["reason"],
        json!("terminated")
    );
}

// ================================================================ Madde 7: claim devri (reassign)

/// self__creditAnalyst node'una reassign kuralı ekleyen golden varyantı: bu
/// node'daki claim'i yalnız branchManager (amir) devredebilir.
fn golden_with_reassign() -> Wfd {
    let mut wfd = golden();
    wfd.nodes.get_mut("self__creditAnalyst").unwrap().reassign = Some(CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: Some(vec!["branchManager".into()]),
        c_u: None,
    });
    wfd
}

#[tokio::test]
async fn reassign_by_authorized_manager_to_eligible_target() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu); // mevcut sahip
    let mgr = manager(orgu); // amir (reassign kuralına uyar)
    let target = analyst(orgu); // node c_a'ya uygun yeni sahip
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entry = engine
        .reassign(&wfd, &wfes, &mgr, Some(&target), None, Utc::now())
        .await
        .expect("yetkili amir uygun hedefe devredebilmeli");

    assert_eq!(entry.action, "reassign");
    assert_eq!(
        entry.actor.user_id, mgr.user_id,
        "marker aktörü amir olmalı"
    );
    let input = entry.input.as_ref().unwrap();
    assert_eq!(input["from"], json!(owner.user_id.to_string()));
    assert_eq!(input["to"], json!(target.user_id.to_string()));
}

#[tokio::test]
async fn reassign_to_pool_writes_unclaim_marker() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let mgr = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entry = engine
        .reassign(&wfd, &wfes, &mgr, None, None, Utc::now())
        .await
        .expect("amir havuza bırakabilmeli");

    assert_eq!(entry.action, "unclaim");
    let input = entry.input.as_ref().unwrap();
    assert_eq!(input["from"], json!(owner.user_id.to_string()));
    assert_eq!(input["to"], Value::Null, "havuza bırakmada hedef null");
}

#[tokio::test]
async fn reassign_by_unauthorized_actor_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let intruder = analyst(orgu); // reassign kuralı branchManager ister — analyst uymaz
    let target = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let err = engine
        .reassign(&wfd, &wfes, &intruder, Some(&target), None, Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Unauthorized),
        "beklenen Unauthorized, gelen: {err:?}"
    );
}

#[tokio::test]
async fn reassign_on_node_without_rule_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    // reassign kuralı EKLENMEMİŞ düz golden — devir tamamen kapalı.
    let wfd = golden();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let mgr = manager(orgu);
    let target = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let err = engine
        .reassign(&wfd, &wfes, &mgr, Some(&target), None, Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Unauthorized),
        "kural yoksa devir kapalı olmalı"
    );
}

#[tokio::test]
async fn reassign_to_ineligible_target_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let mgr = manager(orgu);
    let target = clerk(orgu); // branchClerk — node c_a creditAnalyst'e uymaz
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let err = engine
        .reassign(&wfd, &wfes, &mgr, Some(&target), None, Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::TargetNotEligible),
        "beklenen TargetNotEligible, gelen: {err:?}"
    );
}

#[tokio::test]
async fn reassign_on_terminal_wfe_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let mgr = manager(orgu);
    let target = analyst(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", Some(Uuid::new_v4()), start_input());
    wfes.status = WfeStatus::Terminal;

    let err = engine
        .reassign(&wfd, &wfes, &mgr, Some(&target), None, Utc::now())
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::WfeTerminal),
        "terminal WFE'de devir reddedilmeli"
    );
}

#[tokio::test]
async fn required_input_sent_as_null_is_rejected() {
    // WOR-70b: `required` gönderilmek zorunda VE null olamaz.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["credit_info"] = Value::Null;

    let err = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &input,
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::InvalidInput(m) if m.contains("null olamaz")),
        "{err}"
    );
}

#[tokio::test]
async fn required_input_allows_null_in_undeclared_subfield() {
    // Null denetimi YALNIZ bildirilen yola bakar: `required: ["applicant"]` ile
    // applicant.income null gelebilir (income ayrıca required bildirilmemiş).
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["applicant"]["income"] = Value::Null;

    let new = engine
        .start(
            &golden(),
            &actor,
            Uuid::nil(),
            None,
            &input,
            Uuid::new_v4(),
            None,
        )
        .await
        .expect("bildirilmemiş alt alanın null'u geçerli");
    assert_eq!(new.initial_dynctx["applicant"]["income"], Value::Null);
    assert_eq!(new.initial_dynctx["applicant"]["name"], json!("Ayşe Yılmaz"));
}

#[tokio::test]
async fn optional_input_sent_as_value_is_written() {
    // Karşı taraf: opsiyonel girdi GÖNDERİLDİĞİNDE değeri ctx'e yazılır.
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    let orgu = Uuid::new_v4();
    let m = manager(orgu);
    let wfes = wfes_at(
        "self__branchManager",
        Some(m.user_id),
        json!({
            "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
            "credit_info": {"amount_requested": 5000}
        }),
    );

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "approve", "internal_notes": "müdür notu"}),
            None,
        )
        .await
        .expect("aksiyon uygulanmalı");
    assert_eq!(commit.new_dynctx["internal_notes"], json!("müdür notu"));
}

// ---- WOR-73: ZEN join koşulu (join_mode: expr) --------------------------------

/// Kural: "(finans VE hukuk) YA DA İK" — sayıyla ifade EDİLEMEZ (2-of-3 değil:
/// finans+İK ikilisi yetmez, finans+hukuk yeter, tek başına İK yeter).
const JOIN_EXPR: &str =
    "($branches.self__financeApprover and $branches.self__legalApprover) or $branches.self__hrApprover";

fn expr_wfes(branches: Vec<BranchState>) -> Wfes {
    let mut w = parallel_wfes(branches, join_node(), parallel_ctx());
    w.join_rule = JoinRule::Expr(JOIN_EXPR.into());
    w
}

async fn apply_approve(wfes: &Wfes, actor: &Actor, node: &str) -> CommitOutcome {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
    };
    engine
        .apply(&paralel(), wfes, actor, "approve", &json!({}), Some(node))
        .await
        .expect("aksiyon uygulanmalı")
        .outcome
}

/// İfade henüz `false` (yalnız finans vardı) → ara varış.
#[tokio::test]
async fn expr_join_incomplete_arrival_is_branch_arrived() {
    let fin = actor_with_role("financeApprover");
    let wfes = expr_wfes(vec![
        branch(
            "self__financeApprover",
            BranchStatus::Active,
            Some(fin.user_id),
        ),
        branch("self__legalApprover", BranchStatus::Active, None),
        branch("self__hrApprover", BranchStatus::Active, None),
    ]);
    let outcome = apply_approve(&wfes, &fin, "self__financeApprover").await;
    match outcome {
        CommitOutcome::BranchArrived {
            from_node,
            arrived_entries,
        } => {
            assert_eq!(from_node, "self__financeApprover");
            // Karar bu küme üzerinde verildi — adapter kilit altında bunu doğrular.
            assert_eq!(arrived_entries, vec!["self__financeApprover".to_string()]);
        }
        other => panic!("BranchArrived beklendi: {other:?}"),
    }
}

/// finans ZATEN varmış, hukuk varıyor → ifade `true`, geride İK kolu kaldığı için
/// quorum_collapse (kalan kol iptal).
#[tokio::test]
async fn expr_join_completes_when_and_side_satisfied() {
    let legal = actor_with_role("legalApprover");
    let wfes = expr_wfes(vec![
        branch("self__financeApprover", BranchStatus::Arrived, None),
        branch(
            "self__legalApprover",
            BranchStatus::Active,
            Some(legal.user_id),
        ),
        branch("self__hrApprover", BranchStatus::Active, None),
    ]);
    let outcome = apply_approve(&wfes, &legal, "self__legalApprover").await;
    match outcome {
        CommitOutcome::JoinComplete {
            quorum_collapse,
            arrived_entries,
            next,
            ..
        } => {
            assert!(quorum_collapse, "geride İK kolu kaldı → iptal edilecek");
            assert_eq!(
                arrived_entries,
                vec![
                    "self__financeApprover".to_string(),
                    "self__legalApprover".to_string()
                ]
            );
            assert!(matches!(*next, CommitOutcome::MoveTo { ref node } if node == "self__resultCoordinator"));
        }
        other => panic!("JoinComplete beklendi: {other:?}"),
    }
}

/// İK tek başına yeter (`or` tarafı) — iki kardeş kol hâlâ aktifken join dolar.
/// Aynı senaryo K-of-N ile ifade EDİLEMEZ: eşik 1 olsaydı finans tek başına da
/// yeterdi, eşik 2 olsaydı İK tek başına yetmezdi.
#[tokio::test]
async fn expr_join_or_side_completes_alone() {
    let hr = actor_with_role("hrApprover");
    let wfes = expr_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
        branch("self__hrApprover", BranchStatus::Active, Some(hr.user_id)),
    ]);
    let outcome = apply_approve(&wfes, &hr, "self__hrApprover").await;
    match outcome {
        CommitOutcome::JoinComplete {
            quorum_collapse,
            arrived_entries,
            ..
        } => {
            assert!(quorum_collapse);
            assert_eq!(arrived_entries, vec!["self__hrApprover".to_string()]);
        }
        other => panic!("JoinComplete beklendi: {other:?}"),
    }
}

/// Finans + İK varmış, SON kol (hukuk) da varıyor ama ifade... `or $branches.hr`
/// yüzünden zaten `true` olur. Tatmin edilemezliği görmek için yalnız `and`
/// tarafını isteyen bir kural kurup İK'yı hiç varmamış gösteriyoruz.
#[tokio::test]
async fn expr_join_unsatisfiable_at_last_arrival_fails_loudly() {
    let fin = actor_with_role("financeApprover");
    let mut wfes = expr_wfes(vec![
        branch(
            "self__financeApprover",
            BranchStatus::Active,
            Some(fin.user_id),
        ),
        branch("self__legalApprover", BranchStatus::Cancelled, None),
        branch("self__hrApprover", BranchStatus::Cancelled, None),
    ]);
    // Kural yalnız hukuk kolunu istiyor; o kol iptal edilmiş → hiç dolamaz.
    wfes.join_rule = JoinRule::Expr("$branches.self__legalApprover".into());
    let outcome = apply_approve(&wfes, &fin, "self__financeApprover").await;
    match outcome {
        CommitOutcome::Failed { end_response } => {
            assert_eq!(end_response["reason"], json!("WFD.JoinUnsatisfied"));
            assert_eq!(end_response["join_rule"], json!("expr"));
        }
        other => panic!("Failed(WFD.JoinUnsatisfied) beklendi: {other:?}"),
    }
}

/// Kol İÇİNDE hareket eden kolun kimliği DEĞİŞMEZ: ifade giriş node'unu görür.
/// (`branch_node` hareketle değişir — kimlik `entry_node`'dur.)
#[tokio::test]
async fn expr_join_identifies_branch_by_entry_node_after_move() {
    let hr = actor_with_role("hrApprover");
    let mut moved = branch("self__hrApprover", BranchStatus::Active, Some(hr.user_id));
    // Kol İK girişinden başlayıp başka bir node'a taşınmış olsun.
    moved.branch_node = "self__hrApprover".into();
    moved.entry_node = "self__hrApprover".into();
    let wfes = expr_wfes(vec![
        branch("self__financeApprover", BranchStatus::Active, None),
        branch("self__legalApprover", BranchStatus::Active, None),
        moved,
    ]);
    let outcome = apply_approve(&wfes, &hr, "self__hrApprover").await;
    match outcome {
        CommitOutcome::JoinComplete {
            arrived_entries, ..
        } => assert_eq!(arrived_entries, vec!["self__hrApprover".to_string()]),
        other => panic!("JoinComplete beklendi: {other:?}"),
    }
}
