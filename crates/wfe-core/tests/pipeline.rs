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
    AutoexecDef, AutoexecType, COrgu, CaGrantRule, CandidateActor, ClaimTimeout, EscalationStep,
    GlobalAction, JoinRule, WfAdminRule, Wfd, WfesEffects, Wft, WftTarget,
};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::valid::{self, ValidRules};
use wfe_core::v22::wfah_kind::parse_marker;
use wfe_core::v22::pipeline::{ClaimCheck, ClaimTimeoutOutcome, Engine};
use wfe_core::v22::ports::{
    AutoexecRunner, BranchState, BranchStatus, CollapseCause, CommitOutcome, ExecEnv, ExecFailure,
    Wfes,
};

const FIXTURE: &str = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");

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
            orgu_id: if anchor.is_nil() {
                Uuid::new_v4()
            } else {
                anchor
            },
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

    /// REST sonucunu HAM verir — dış sistemin şemaya uymayan bir şey döndürdüğü
    /// vakaları (kapı B) kurmak için. `calc` normal davranır.
    fn with_rest_result(rest: Value, within_limit: bool) -> Self {
        Self {
            rest: RestBehavior::Ok(rest),
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
    wfes_at_visited(node, assigned, ctx, vec![])
}

/// K-2: geri gönderme testleri için "bu WFE şu node'lardan geçti" kurulumu.
/// `wfes_at` bunun boş-geçmişli hâlidir — çekirdek `visited_nodes` fonksiyonu
/// `current_node`u zaten kümeye koyduğu için geri gönderme DIŞINDAKİ testler
/// etkilenmez.
fn wfes_at_visited(node: &str, assigned: Option<Uuid>, ctx: Value, visited: Vec<String>) -> Wfes {
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    // R02: start satırı HAREKET satırıdır — gerçek motor `stamp_movement` ile
    // `to_node`'u yazar (start commit'i, `pipeline.rs`). Kısayol `Wfah::push` alanı
    // `None` bıraktığı için satır burada elle kurulur: `to_node` olmadan
    // `node_entered_at` taban bulamaz ve escalation testleri gerçeğe UYMAYAN bir
    // defter üzerinde koşardı.
    let wfah = Wfah(vec![WfahEntry {
        seq: 1,
        action: "start".into(),
        actor: system,
        input: None,
        applied_at: Utc::now(),
        from_node: None,
        to_node: Some(node.into()),
        branch_entry: None,
        branch_round: None,
    }]);
    let created_at = wfah.entries()[0].applied_at;
    Wfes {
        wfe_id: Uuid::new_v4(),
        orgtnt_id: Uuid::nil(),
        environment_id: None,
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah,
        status: WfeStatus::Active,
        visited_nodes: visited,
        current_node: Some(node.into()),
        end_terminal: None,
        assigned_to: assigned,
        end_response: None,
        deadline: None,
        claimed_at: assigned.map(|_| created_at),
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
        origin_orgu_id: None,
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
        env: Default::default(),
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
        env: Default::default(),
    };
    let actor = clerk(Uuid::new_v4());

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    // v2.3 (`Ç7`): `start[]` yalnız `{id, action}`; effects start AKSİYONUNDA.
    v["actions"]["create_application"]["wfes_effects"] =
        json!({ "set": { "initiated_by": "$actor" } });
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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

// ======================================= WFAH-çapalı c_orgu (2026-08-11 regresyonu)
//
// `{from: {wfah: "<aksiyon>", field: "actor.orgu"}}` çapası, VARILAN node'un adayını
// çözerken o node'a girişi ÜRETEN aksiyonu görebilmelidir. Geçmiş §7 atomikliği gereği
// commit'ten önce yazılmaz — kayıt yalnız staged listede durur ve `resolve_wft`e ayrıca
// verilmezse çapa çözülemez, `resolve_c_orgu` BOŞ küme döner (aktörün birimine düşmez)
// ve node ADAYSIZ açılır: portalda "c_a boş", kimse claim edemez, "uygun aktör oluştur"
// da hangi (birim, rol) için olduğunu bilemez.

/// Start: "başlatanın biriminin analisti" kuralı, start kaydını görmek ZORUNDA.
#[tokio::test]
async fn start_target_resolves_wfah_anchor_to_start_actor_orgu() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["nodes"]["self__creditAnalyst"]["c_a"]["c_orgu"] = json!({
        "from": {"wfah": "create_application", "field": "actor.orgu"},
        "traverse": "self"
    });
    let wfd = Wfd::from_value(v).unwrap();

    let orgu = Uuid::new_v4();
    let new = engine
        .start(
            &wfd,
            &clerk(orgu),
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();

    assert!(
        new.resolved_c_a
            .iter()
            .any(|c| c.orgu_id == Some(orgu) && c.role == "creditAnalyst"),
        "start aksiyonuna çapalı c_orgu başlatanın birimine çözülmeli: {:?}",
        new.resolved_c_a
    );
}

/// Apply: çapa, geçişi üreten aksiyonu (henüz commit edilmemiş kaydı) görmeli.
#[tokio::test(start_paused = true)]
async fn apply_target_resolves_wfah_anchor_to_applying_actor_orgu() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(650, "C", false); // within_limit false → default node
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["nodes"]["self__branchManager"]["c_a"] = json!({
        "c_orgu": {"from": {"wfah": "analyst_approve", "field": "actor.orgu"}, "traverse": "self"},
        // Rol `listable[]`teki hiçbir kuralda GEÇMEMELİ — geçseydi aday yalnız o
        // union'dan gelip çapa çözülmese de assertion tutardı (test ayırt etmezdi).
        "c_r": ["wfahAnchoredManager"]
    });
    let wfd = Wfd::from_value(v).unwrap();

    let orgu = Uuid::new_v4();
    let a = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 90000}}),
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "{:?}",
        commit.outcome
    );
    assert!(
        commit
            .resolved_c_a
            .iter()
            .any(|c| c.orgu_id == Some(orgu) && c.role == "wfahAnchoredManager"),
        "aksiyona çapalı c_orgu aksiyonu alanın birimine çözülmeli: {:?}",
        commit.resolved_c_a
    );
}

/// Ayrımın diğer yarısı: KOŞUL değerlendirmesi o aksiyonu HÂLÂ görmez. `$prev` "bir
/// önceki aksiyon" demektir ve yayınlanmış `count($wfah, ...)` eşikleri uygulanan
/// aksiyonu erken saymamalıdır — aday çözümüne verilen geçmiş buraya SIZMAMALI.
#[tokio::test(start_paused = true)]
async fn wft_conditions_do_not_see_the_action_being_applied() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(650, "C", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["actions"]["analyst_approve"]["wft"] = json!({
        "conditions": [{"when": "$prev.action == 'analyst_approve'", "terminal": "terminal_rejected"}],
        "default": {"node": "self__branchManager"}
    });
    let wfd = Wfd::from_value(v).unwrap();

    let a = analyst(Uuid::new_v4());
    // wfes_at geçmişi tek "start" kaydıdır → $prev.action == "start".
    let wfes = wfes_at("self__creditAnalyst", Some(a.user_id), start_input());

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &a,
            "analyst_approve",
            &json!({"credit_info": {"amount_requested": 90000}}),
            None,
            None,
        )
        .await
        .unwrap();

    assert!(
        matches!(&commit.outcome, CommitOutcome::MoveTo { node } if node == "self__branchManager"),
        "$prev uygulanan aksiyona kaymamalı: {:?}",
        commit.outcome
    );
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({}),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidInput(_)), "{err}");
}

// v2.3 (`Ç5` + `Ç10`): **İLK-MATCH SEÇİMİ ÖLDÜ** — `first_match_wfd` ve
// `first_matching_when_wins_in_array_order` SİLİNDİ. v2.2'de aynı `(node, action)`
// çifti için birden çok `transitions[]` girdisi olabiliyor, motor dizi sırasında
// ilk `when`i tutanı seçiyordu. Artık kimlik map anahtarıdır: aday YA TEKTİR ya da
// yoktur, `when` false dönerse ikinci bir adaya DÜŞÜLMEZ. Testin kurduğu durum
// (iki kayıt, aynı ad) v2.3'te kurulamıyor.
//
// ⚠️ `Ç10`un yerine geçen güvence — "`when` false ⇒ aksiyon alınamaz, yedek yok" —
// bu dosyada HENÜZ TEST EDİLMİYOR. Ayrı kalem.

// ================================================================ NoConditionMatched (M3)

#[tokio::test(start_paused = true)]
async fn conditional_without_default_and_no_match_errors() {
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    v["actions"]["manager_decide"]["wft"] = json!({
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let actions = engine
        .possible_actions(&golden(), &wfes, &m, None)
        .await
        .unwrap();
    assert_eq!(
        actions
            .iter()
            .map(|a| a.action.as_str())
            .collect::<Vec<_>>(),
        vec!["manager_decide"]
    );
    // Düz aksiyon: hedef seçimi YOK (alan API'de de hiç çıkmaz).
    assert!(actions[0].targets.is_none());

    // owner olmayan boş liste alır
    let other = manager(Uuid::new_v4());
    let actions = engine
        .possible_actions(&golden(), &wfes, &other, None)
        .await
        .unwrap();
    assert!(actions.is_empty());
}

// ================================ GERİ GÖNDER — `wft: {targets}` runtime (api-contract-v2)
//
// Tasarım-zamanı kuralları `tests/validator.rs`te. Buradakiler ÇALIŞMA ANI sözleşmesi:
// hedefi artık aksiyon anahtarı değil, isteğin `target` alanı taşır. Menü aksiyonun
// PARÇASIDIR — bu yüzden "hedef verilmedi" ile "hedef uydurulmuş" AYRI hatalardır,
// ikisi de commit'e hiç ulaşmaz.

/// `t_manager_decide`ı geri göndermeye çevirir (validator testindeki `with_send_back`in
/// ikizi): aksiyon REZERVE `send_back`, hedef menüsü `self__creditAnalyst` ve hedefin
/// KENDİ etiketi. Yetim kalan `terminal_rejected` düşürülür — hedefler node'dur,
/// terminal hedeflenemez. Aksiyon katalog girdisi taban aksiyondan kopyalanır: girdi
/// bildirimi (`manager_decision`) aynı kalsın ki `wfes_effects` sözleşmesi (WOR-70)
/// bozulmasın.
fn golden_with_send_back() -> Wfd {
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    // v2.3 (`Ç5`): kimlik ile yönlendirme tek kayıtta — müdür aksiyonunun YERİNE
    // geçen bir `send_back` kaydı. Girdi bildirimi ve `wfes_effects` taban aksiyondan
    // kopyalanır (WOR-70 sözleşmesi bozulmasın); değişen yalnız ad ve `wft`.
    let mut base = v["actions"]["manager_decide"].clone();
    let o = base.as_object_mut().unwrap();
    // Gösterim metni hedefin KENDİ etiketinden gelir — aksiyona `label` YAZILMAZ.
    o.remove("label");
    o.insert(
        "wft".into(),
        json!({ "targets": [{"node": "self__creditAnalyst", "label": "Başa Gönder"}] }),
    );
    v["actions"]["send_back"] = base;
    v["actions"]
        .as_object_mut()
        .unwrap()
        .remove("manager_decide");
    if let Some(terminals) = v["terminals"].as_array_mut() {
        terminals.retain(|t| t["id"] != json!("terminal_rejected"));
    }
    Wfd::from_value_checked(v).expect("geri gönderme belgesi şema kapısından geçmeli")
}

fn send_back_engine_parts() -> (MockOrg, MockRunner) {
    (
        MockOrg {
            role_assigned: true,
        },
        MockRunner::ok(0, "-", false),
    )
}

#[tokio::test]
async fn send_back_moves_to_the_chosen_target() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // K-2: bu WFE analist havuzundan geçip müdüre geldi — geri gönderme hedefi
    // ancak UĞRANMIŞ bir node olabilir.
    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    let commit = engine
        .apply(
            &golden_with_send_back(),
            &wfes,
            &m,
            "send_back",
            &json!({"manager_decision": "reject"}),
            None,
            Some("self__creditAnalyst"),
        )
        .await
        .expect("geçerli hedef uygulanmalı");

    match commit.outcome {
        CommitOutcome::MoveTo { ref node } => assert_eq!(node, "self__creditAnalyst"),
        ref other => panic!("seçilen hedefe gitmeliydi: {other:?}"),
    }
    // Seçim CONTEXT'E yazılmaz — `target` bir action input DEĞİLDİR.
    assert_eq!(commit.new_dynctx.get("target"), None);
    assert_eq!(commit.new_dynctx.get("hedef"), None);
    // WFAH'a yazılan ad TABAN aksiyondur; hedef anahtara kodlanmaz (`__gt__` kalktı) —
    // yayınlanmış akışların `count($wfah, #.action == "send_back")` sayımı bozulmaz.
    assert!(commit.wfah_entries.iter().any(|e| e.action == "send_back"));
}

#[tokio::test]
async fn send_back_without_a_target_is_rejected() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // K-2: bu WFE analist havuzundan geçip müdüre geldi — geri gönderme hedefi
    // ancak UĞRANMIŞ bir node olabilir.
    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    let err = engine
        .apply(
            &golden_with_send_back(),
            &wfes,
            &m,
            "send_back",
            &json!({"manager_decision": "reject"}),
            None,
            None,
        )
        .await
        .expect_err("hedefsiz geri gönderme uygulanmamalı");
    assert!(
        matches!(err, EngineError::TargetRequired),
        "beklenen TargetRequired, gelen: {err:?}"
    );
}

#[tokio::test]
async fn send_back_with_an_unlisted_target_is_rejected() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // K-2: bu WFE analist havuzundan geçip müdüre geldi — geri gönderme hedefi
    // ancak UĞRANMIŞ bir node olabilir.
    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    // `self__branchManager` belgede GERÇEK bir node — ama bu aksiyonun menüsünde YOK.
    // Kapı "node var mı"ya değil "menüde mi"ye bakmalı, aksi halde istemci istediği
    // node'a atlayarak grafı dolanırdı.
    let err = engine
        .apply(
            &golden_with_send_back(),
            &wfes,
            &m,
            "send_back",
            &json!({"manager_decision": "reject"}),
            None,
            Some("self__branchManager"),
        )
        .await
        .expect_err("menüde olmayan hedef reddedilmeli");
    assert!(
        matches!(err, EngineError::TargetInvalid(_)),
        "beklenen TargetInvalid, gelen: {err:?}"
    );
}

#[tokio::test]
async fn a_plain_action_rejects_a_target() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // K-2: bu WFE analist havuzundan geçip müdüre geldi — geri gönderme hedefi
    // ancak UĞRANMIŞ bir node olabilir.
    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    // Sessizce YOK SAYMAK yanlış olurdu: istemci hedef seçtiğini sanır, motor
    // başka yere götürür. Açık hata, sessiz sapmadan iyidir.
    let err = engine
        .apply(
            &golden(),
            &wfes,
            &m,
            "manager_decide",
            &json!({"manager_decision": "reject"}),
            None,
            Some("self__creditAnalyst"),
        )
        .await
        .expect_err("düz aksiyonda hedef reddedilmeli");
    assert!(
        matches!(err, EngineError::TargetUnexpected),
        "beklenen TargetUnexpected, gelen: {err:?}"
    );
}

#[tokio::test]
async fn possible_actions_offers_the_target_menu() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // K-2: bu WFE analist havuzundan geçip müdüre geldi — geri gönderme hedefi
    // ancak UĞRANMIŞ bir node olabilir.
    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    let actions = engine
        .possible_actions(&golden_with_send_back(), &wfes, &m, None)
        .await
        .unwrap();
    let glb = actions
        .iter()
        .find(|a| a.action == "send_back")
        .expect("geri gönderme aksiyonu listede olmalı");
    let targets = glb
        .targets
        .as_deref()
        .expect("hedef menüsü aksiyonun parçasıdır");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].node, "self__creditAnalyst");
    // Hedefin KENDİ etiketi çekirdekten HAM geçer — çözüm (`node_label`'a düşme)
    // adapter'ın işi, çekirdek gösterim üretmez.
    assert_eq!(targets[0].label.as_deref(), Some("Başa Gönder"));
}

// ---- K-2: hedef menüsü UĞRANMIŞ node'larla kesişir (2026-08-21) ----

/// Belgede hedef var ama WFE o node'a HİÇ uğramadı → menüde ÇIKMAZ. Uğranmamış bir
/// node'a "geri" göndermek geri gönderme değil ileri atlamadır.
#[tokio::test]
async fn unvisited_targets_are_dropped_from_the_menu() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // Geçmiş BOŞ: yalnız bulunulan node bilinir. Menüdeki tek hedef
    // (`self__creditAnalyst`) uğranmadığı için aksiyon HİÇ sunulmaz.
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let actions = engine
        .possible_actions(&golden_with_send_back(), &wfes, &m, None)
        .await
        .unwrap();
    assert!(
        !actions.iter().any(|a| a.action == "send_back"),
        "uğranmamış tek hedefli menü boş kalır → aksiyon sunulmaz: {actions:?}"
    );
}

/// Menü boş kalınca aksiyon SUNULMAZ ama apply da kabul ETMEZ — iki kapı aynı kümeye
/// bakar. İstemci menüyü atlayıp doğrudan istek atarsa hedef reddedilir.
#[tokio::test]
async fn apply_rejects_an_unvisited_target() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    let wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());

    let err = engine
        .apply(
            &golden_with_send_back(),
            &wfes,
            &m,
            "send_back",
            &json!({"manager_decision": "reject"}),
            None,
            Some("self__creditAnalyst"),
        )
        .await
        .expect_err("uğranmamış hedef reddedilmeli");
    assert!(
        matches!(err, EngineError::TargetInvalid(_)),
        "beklenen TargetInvalid, gelen: {err:?}"
    );
}

/// START node'u `visited_nodes` listesinde YOKTUR (start satırının `from_node`'u NULL,
/// `to_node`'u ilk havuzdur) — çekirdek onu `wfd.start[]`ten ekler. "Başa gönder"in
/// çalışması buna bağlıdır.
#[tokio::test]
async fn the_start_node_counts_as_visited() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());
    // Menü YALNIZ start node'unu gösteriyor; geçmiş listesi BOŞ.
    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    let start_action = v["start"][0]["action"].as_str().unwrap().to_string();
    // v2.3 (`Ç7`): başlatan node `start[]`te değil, start aksiyonunun `from`unda.
    let start_node = v["actions"][&start_action]["from"]
        .as_str()
        .unwrap()
        .to_string();
    let mut base = v["actions"]["manager_decide"].clone();
    base.as_object_mut().unwrap().insert(
        "wft".into(),
        json!({ "targets": [{"node": start_node, "label": "Başa Gönder"}] }),
    );
    v["actions"]["send_back"] = base;
    v["actions"]
        .as_object_mut()
        .unwrap()
        .remove("manager_decide");
    if let Some(terminals) = v["terminals"].as_array_mut() {
        terminals.retain(|t| t["id"] != json!("terminal_rejected"));
    }
    let wfd = Wfd::from_value_checked(v).expect("belge şema kapısından geçmeli");

    // WFAH'ın ilk kaydı start aksiyonudur — çekirdek start node'unu oradan bulur.
    let mut wfes = wfes_at("self__branchManager", Some(m.user_id), start_input());
    wfes.wfah = Wfah::empty().push(
        start_action,
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "system".into(),
        },
        None,
    );

    let actions = engine
        .possible_actions(&wfd, &wfes, &m, None)
        .await
        .unwrap();
    let sb = actions
        .iter()
        .find(|a| a.action == "send_back")
        .expect("başa gönder sunulmalı");
    let targets = sb.targets.as_deref().expect("menü");
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].node, start_node);
    assert_eq!(targets[0].label.as_deref(), Some("Başa Gönder"));
}

/// Kısmi kesişim: uğranmış hedef KALIR, uğranmamış DÜŞER; belgedeki SIRA korunur.
#[tokio::test]
async fn the_menu_keeps_document_order_and_drops_only_the_unvisited() {
    let (org, runner) = send_back_engine_parts();
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let m = manager(Uuid::new_v4());

    let mut v: Value = serde_json::from_str(FIXTURE).unwrap();
    // Uğranmamış bir hedef gerek: golden'da öyle bir node yok, ekliyoruz. Geri
    // gönderme hedefi gerçek bir çıkış kenarıdır, o yüzden node erişilebilir sayılır;
    // çıkışını da kendi aksiyonu verir.
    v["nodes"]["self__ikinciAnalist"] = json!({
        "label": "İkinci Analist",
        "c_a": {"c_orgu": "self", "c_r": ["seniorAnalyst"]}
    });
    v["actions"]["ikinci_analiz"] = json!({
        "input": {"required": [], "optional": []},
        "from": "self__ikinciAnalist",
        "wft": {"node": "self__branchManager"}
    });
    let mut base = v["actions"]["manager_decide"].clone();
    // Belge sırası: HİÇ uğranmamış → uğranmış. Süzgeç sırayı değiştirmemeli.
    base.as_object_mut().unwrap().insert(
        "wft".into(),
        json!({
            "targets": [
                {"node": "self__ikinciAnalist", "label": "İkinci Analiste Gönder"},
                {"node": "self__creditAnalyst", "label": "Analiste Gönder"}
            ]
        }),
    );
    v["actions"]["send_back"] = base;
    v["actions"]
        .as_object_mut()
        .unwrap()
        .remove("manager_decide");
    if let Some(terminals) = v["terminals"].as_array_mut() {
        terminals.retain(|t| t["id"] != json!("terminal_rejected"));
    }
    let wfd = Wfd::from_value_checked(v).expect("belge şema kapısından geçmeli");

    let wfes = wfes_at_visited(
        "self__branchManager",
        Some(m.user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );
    let actions = engine
        .possible_actions(&wfd, &wfes, &m, None)
        .await
        .unwrap();
    let targets = actions
        .iter()
        .find(|a| a.action == "send_back")
        .expect("aksiyon sunulmalı")
        .targets
        .as_deref()
        .expect("menü");
    assert_eq!(targets.len(), 1, "uğranmamış hedef düşmeli: {targets:?}");
    assert_eq!(targets[0].node, "self__creditAnalyst");
    assert_eq!(targets[0].label.as_deref(), Some("Analiste Gönder"));
}

// ================================================================ escalation (M6)

#[tokio::test]
async fn escalation_fires_after_sla_and_keeps_the_work_in_place() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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

    // fire → effects uygulanır (assigned olsa bile çalışır), İŞ YERİNDE KALIR.
    //
    // v2.3 (`K10` + `Ç1-EK`): kademe devir DEĞİL yetki genişlemesidir. Eskiden burada
    // `MoveTo{self__branchManager}` bekleniyordu; artık müdür analistin havuzuna
    // EKLENİR ve iş analist node'unda durur.
    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "self__creditAnalyst"),
        "outcome: {:?}",
        commit.outcome
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
        env: Default::default(),
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
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "self__creditAnalyst"),
        "outcome: {:?}",
        commit.outcome
    );
    // Çözüm wfah'taki son insan aktörüne çapalanır — listable `self` buna göre çözülür.
    assert!(commit
        .resolved_c_a
        .iter()
        .any(|c| c.orgu_id == Some(human_orgu)));
    // Audit izi DEĞİŞMEZ: marker yine nil-orgu'lu saf `system` aktörüdür.
    assert_eq!(commit.wfah_entries[0].actor.role, "system");
    assert!(commit.wfah_entries[0].actor.orgu_id.is_nil());
}

// v2.3 (`K13`): `claim_timeout_move_resolves_anchored_listable_via_wfah_actor`
// SİLİNDİ — `ClaimTimeoutOutcome::Move` yolu ÖLDÜ. Süre dolduğunda yapılan tek şey
// claim'i bırakmak; devir hedefi olmadığı için çözülecek bir `listable`/`c_a` da
// yok, dolayısıyla çapa sorusu bu yolda artık sorulmuyor. Havuz node'un kendi
// `c_a`sıdır ve değişmez.

#[tokio::test]
async fn fired_escalation_step_does_not_refire() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
        env: Default::default(),
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
            // v2.3 (`Ç9`): hedef yerine GRANT — iş adımda kalır, havuz genişler.
            grant: CaGrantRule {
                c_a: serde_json::from_value(json!({"c_orgu": "self", "c_r": ["branchManager"]}))
                    .unwrap(),
                when: None,
            },
        });

    let t0 = Utc::now();
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    // Kontrollü WFAH: node girişi T0 (HAREKET satırı, `to_node` dolu — R02 tabanı);
    // adım 0 marker'ı T0+3g (gün sonra ateşlendi) ve `to_node` TAŞIMAZ.
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.wfah = Wfah(vec![
        WfahEntry {
            seq: 1,
            action: "start".into(),
            actor: system.clone(),
            input: None,
            applied_at: t0,
            from_node: None,
            to_node: Some("self__creditAnalyst".into()),
            branch_entry: None,
            branch_round: None,
        },
        WfahEntry {
            seq: 2,
            action: "escalate:self__creditAnalyst:0".into(),
            actor: system.clone(),
            input: None,
            applied_at: t0 + Duration::days(3),
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
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
    // v2.3 (`Ç7+Ç8`): start gövdesi aksiyon kaydında.
    let start_action = wfd.start[0].action.clone();
    wfd.actions
        .get_mut(&start_action)
        .expect("start aksiyonu")
        .wft = Wft::Node {
        node: "type_branch__branchClerk".into(),
    };

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
            // v2.3 (`Ç9`): hedef yerine GRANT — iş adımda kalır, havuz genişler.
            grant: CaGrantRule {
                c_a: serde_json::from_value(json!({"c_orgu": "self", "c_r": ["branchManager"]}))
                    .unwrap(),
                when: None,
            },
        });

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "type_branch__branchClerk"),
        "outcome: {:?}",
        commit.outcome
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
/// v2.3 (K13 + K19): `wft` parametresi KALKTI — claim timeout artık yalnız claim'i
/// bırakır, hedefi yok.
fn golden_with_claim_timeout(after: &str) -> Wfd {
    let mut wfd = golden();
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .claim_timeout = Some(ClaimTimeout {
        after: after.into(),
        wfes_effects: None,
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
        env: Default::default(),
    };
    let wfd = golden_with_claim_timeout("PT2H");
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
                "claim_released:self__creditAnalyst"
            );
            assert_eq!(release.wfah_entry.actor.role, "system");
            // wfes_effects yok → ctx satırı yazılmaz
            assert!(release.new_dynctx.is_none());
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

// v2.3: `claim_timeout_due_with_wft_moves_like_escalation` testi SİLİNDİ — konusu (claim timeout'un iş TAŞIMASI) K13 ile öldü

/// WOR-56/SLA-1 (2026-08-03): `collapses_parallel` işaretli olsa bile WFE paralel
/// modda DEĞİLSE bayrak yok sayılır — normal `{node}` devri uygulanır. Aksi halde
/// `resolve_wft` collapse'ı Single modda reddeder ve WFE zaman aşımında kilitlenirdi
/// (aynı node kol içinden de kol dışından da erişilebilir).
// v2.3: `claim_timeout_collapse_flag_ignored_outside_parallel` testi SİLİNDİ — `collapses_parallel` bayrağı K19 ile kalktı

// ---- 2026-07-28: SLA-1 wfes_effects (opsiyonel DynCtx yazımı) ----

/// `golden_with_claim_timeout` + `wfes_effects` — SLA-1 dolduğunda ctx'e yazar.
fn golden_with_claim_timeout_effects(after: &str) -> Wfd {
    let mut wfd = golden_with_claim_timeout(after);
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
        env: Default::default(),
    };
    let wfd = golden_with_claim_timeout_effects("PT2H");
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
            assert_eq!(
                ctx["analyst_approved_at"],
                json!(wfe_core::timestamp::timestamp_string(now))
            );
        }
        ClaimTimeoutOutcome::Move(_) => panic!("wft yokken Release bekleniyordu"),
    }
}

// v2.3: `claim_timeout_move_applies_wfes_effects_before_wft` testi SİLİNDİ — devir yok; effects'in release yolunda uygulanması `claim_timeout_release_applies_wfes_effects` ile zaten sınanıyor

#[tokio::test]
async fn claim_timeout_not_due_without_claim() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let wfd = golden_with_claim_timeout("PT1H");
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
// v2.3: `escalation_without_wft_errors_instead_of_terminating` testi SİLİNDİ — `escalation[].wft` YOK ve `grant` ZORUNLU alan — eksikliği serde'de patlar, runtime hatası diye bir durum kalmadı

/// Hedefi olan adım normal node devri yapar — `Terminated` ASLA üretmez.
#[tokio::test]
async fn escalation_never_terminates_and_keeps_the_work_in_place() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let wfd = golden();
    let wfes = wfes_at("self__creditAnalyst", None, start_input());
    let now = wfes.created_at + Duration::days(3) + Duration::seconds(1);

    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, None)
        .await
        .unwrap();
    // `K10`: escalation WFE'yi ASLA bitirmez — ve v2.3'te (`Ç1-EK`) taşımaz da.
    assert!(
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "self__creditAnalyst"),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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

const PARALLEL_FIXTURE: &str = include_str!("../../../docs/spec/examples/paralel-onay.json");

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
        environment_id: None,
        wfd_id: Uuid::new_v4(),
        wfd_version: 1,
        dynctx: DynCtx(ctx),
        wfah,
        status: WfeStatus::Active,
        visited_nodes: vec![],
        current_node: None,
        end_terminal: None,
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches,
        join_target: Some(join),
        // WOR-72: bu yardımcı AND-join kurar; quorum/expr testleri kendi kuralını verir.
        join_rule: JoinRule::All,
        origin_orgu_id: None,
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
        env: Default::default(),
    };
    let coord = actor_with_role("coordinator");
    let wfes = wfes_at("self__coordinator", Some(coord.user_id), parallel_ctx());

    let commit = engine
        .apply(
            &paralel(),
            &wfes,
            &coord,
            "start_review",
            &json!({}),
            None,
            None,
        )
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
        env: Default::default(),
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
            None,
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
        env: Default::default(),
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
            "finans_onay",
            &json!({}),
            Some("self__financeApprover"),
            None,
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
    assert_eq!(
        wfah_actions(&commit),
        vec!["finans_onay", "_branch_arrived"]
    );
    // Ç3: kol kimliği (`branch_entry`) + varış anındaki konum (`at_node`).
    let arrived = commit.wfah_entries[1].input.as_ref().unwrap();
    assert_eq!(arrived["branch_entry"], json!("self__financeApprover"));
    assert_eq!(arrived["at_node"], json!("self__financeApprover"));
    // Ç4: marker satırı da hangi kolda yazıldığını TAŞIR (kolon karşılığı).
    assert_eq!(
        commit.wfah_entries[1].branch_entry.as_deref(),
        Some("self__financeApprover")
    );
    // varış WFE'yi taşımaz — aday cache boş kalır (kol havuzu T3'te branch satırından)
    assert!(commit.resolved_c_a.is_empty());
}

#[tokio::test]
async fn apply_with_a_hint_that_is_not_an_active_branch_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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

    // v2.3 (`Ç5` + `Ç11`): bu testin ESKİ ilk yarısı — "`approve` üç kolun da
    // transition'ıyla eşleşir, node ipucu yoksa `AmbiguousAction`" — KALDIRILDI.
    // `apply_parallel` adayı `wfd.actions.get(action).filter(|t| t.from == branch_node)`
    // ile buluyor: kayıt TEK, `from` da tekil string (`K3`), dolayısıyla eşleşen kol
    // en fazla BİR tane olabilir. `AmbiguousAction` kolu artık savunma amaçlıdır ve
    // GEÇERLİ bir belgeyle tetiklenemez.
    //
    // geçersiz ipucu: aktif kol değil — bu yarı CANLI
    let err = engine
        .apply(
            &paralel(),
            &wfes,
            &fin,
            "finans_onay",
            &json!({}),
            Some("self__coordinator"),
            None,
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
        env: Default::default(),
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
            "finans_onay",
            &json!({}),
            Some("self__financeApprover"),
            None,
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
            "finans_onay",
            &json!({}),
            Some("self__financeApprover"),
            None,
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
        env: Default::default(),
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
        .apply(&paralel(), &wfes, &hr, "ik_onay", &json!({}), None, None)
        .await
        .unwrap();

    let CommitOutcome::JoinComplete {
        from_node, next, ..
    } = &commit.outcome
    else {
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
    assert_eq!(wfah_actions(&commit), vec!["ik_onay", "_branch_arrived"]);
}

#[tokio::test]
async fn last_branch_arrival_completes_join_to_terminal() {
    // join hedefi terminal olan varyant: kol wft'leri de aynı terminale çözülür,
    // son varışta JoinComplete{next: Terminal} üretilmeli.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["actions"]["start_review"]["wft"]["parallel"]["join"] =
        json!({"terminal": "terminal_approved"});
    for a in ["finans_onay", "hukuk_onay", "ik_onay"] {
        v["actions"][a]["wft"] = json!({"terminal": "terminal_approved"});
    }
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
        .apply(&wfd, &wfes, &hr, "ik_onay", &json!({}), None, None)
        .await
        .unwrap();

    let CommitOutcome::JoinComplete {
        from_node, next, ..
    } = &commit.outcome
    else {
        panic!("JoinComplete bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(from_node, "self__hrApprover");
    let CommitOutcome::Terminal { end_response } = next.as_ref() else {
        panic!("Terminal next bekleniyordu: {next:?}");
    };
    assert_eq!(end_response["status"], json!("approved"));
    assert_eq!(end_response["request_title"], json!("Sunucu alımı"));
    assert_eq!(wfah_actions(&commit), vec!["ik_onay", "_branch_arrived"]);
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
        env: Default::default(),
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
            "hukuk_ret",
            &json!({}),
            Some("self__legalApprover"),
            None,
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
            "hukuk_ret",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let cancel = &commit.wfah_entries[2];
    assert_eq!(
        cancel.input.as_ref().unwrap()["branch_entry"],
        json!("self__financeApprover")
    );
    assert_eq!(
        cancel.input.as_ref().unwrap()["reason"],
        json!("sibling_terminal")
    );
    assert_eq!(cancel.actor.role, "system");
    let superseded = &commit.wfah_entries[3];
    assert_eq!(
        superseded.input.as_ref().unwrap()["branch_entry"],
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
    // finans kolunun ret aksiyonunu collapse-to-node yap (hedef: self__coordinator).
    v["actions"]["finans_ret"]["wft"] = json!({"collapse": {"node": "self__coordinator"}});
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
            "finans_ret",
            &json!({}),
            Some("self__financeApprover"),
            None,
        )
        .await
        .unwrap();

    let CommitOutcome::CollapseTo {
        from_node,
        node,
        cause,
    } = &commit.outcome
    else {
        panic!("CollapseTo bekleniyordu: {:?}", commit.outcome);
    };
    // Tasarımcının `collapse` wft'i — geri gönderme DEĞİL (Ç4-EK/S4).
    assert_eq!(*cause, CollapseCause::Collapse);
    assert_eq!(from_node.as_deref(), Some("self__financeApprover"));
    assert_eq!(node, "self__coordinator");
    // hedef node'un adayları promotion için resolve edilir
    assert!(commit.resolved_c_a.iter().any(|c| c.role == "coordinator"));
    // aktif sibling (legal) iptal marker'ı; hr arrived → superseded (WOR-60);
    // acting kol (finance) hiç marker almaz.
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "finans_ret",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let cancel = &commit.wfah_entries[2];
    assert_eq!(
        cancel.input.as_ref().unwrap()["branch_entry"],
        json!("self__legalApprover")
    );
    assert_eq!(cancel.input.as_ref().unwrap()["reason"], json!("collapsed"));
    assert_eq!(cancel.actor.role, "system");
    // WOR-59: claim'siz kolda alanlar açıkça null (alan HER ZAMAN var)
    assert!(cancel.input.as_ref().unwrap()["claimed_by"].is_null());
    assert!(cancel.input.as_ref().unwrap()["claimed_at"].is_null());
}

// ---- Ç4-EK/S4+S5: geri gönderme kol sınırı ------------------------------------

/// Bir kola geri gönderme MENÜSÜ takar; hedefleri `targets` listesinden gelir.
fn parallel_with_send_back(from: &str, targets: &[&str]) -> Wfd {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    for t in v["transitions"].as_array_mut().unwrap() {
        if t["action"] == json!("reject") && t["from"] == json!(from) {
            t["wft"] = json!({
                "targets": targets.iter().map(|n| json!({"node": n})).collect::<Vec<_>>()
            });
        }
    }
    Wfd::from_value(v).unwrap()
}

/// Ç4-EK/S4 — **ölü kilit senaryosu KAPANDI.** Kol içinden fork ÖNCESİ bir node'a
/// geri gönderme `BranchMoveTo` üretiyordu: kol fork'un dışına oturuyor, paralel mod
/// AÇIK kalıyor, join o kolu sonsuza kadar bekliyordu. Artık `CollapseTo`dur.
#[tokio::test]
async fn send_back_before_the_fork_collapses_instead_of_deadlocking() {
    // `self__coordinator` fork'un KENDİSİDİR (fork alt-grafının dışı).
    let wfd = parallel_with_send_back("self__financeApprover", &["self__coordinator"]);
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let fin = actor_with_role("financeApprover");
    let mut wfes = parallel_wfes(
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
    // K-2: geri gönderme hedefi UĞRANMIŞ olmalı — fork oradan yapıldı.
    wfes.visited_nodes = vec!["self__coordinator".into()];

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &fin,
            "reject",
            &json!({}),
            Some("self__financeApprover"),
            Some("self__coordinator"),
        )
        .await
        .expect("fork öncesine geri gönderme geçerli bir harekettir");

    let CommitOutcome::CollapseTo {
        from_node,
        node,
        cause,
    } = &commit.outcome
    else {
        panic!("CollapseTo bekleniyordu, BranchMoveTo DEĞİL: {:?}", commit.outcome);
    };
    assert_eq!(*cause, CollapseCause::SentBack);
    assert_eq!(from_node.as_deref(), Some("self__financeApprover"));
    assert_eq!(node, "self__coordinator");

    // Kardeşler: aktif olan iptal, varmış olanın onayı geçersiz.
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "reject",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let headline = commit.wfah_entries[1].input.clone().unwrap();
    assert_eq!(headline["kind"], json!("sent_back"));
    assert_eq!(headline["reason"], json!("sent_back"));
    assert_eq!(headline["target"], json!("self__coordinator"));
    assert_eq!(headline["trigger_kind"], json!("branch"));
    assert_eq!(headline["trigger_branch"], json!("self__financeApprover"));
    assert_eq!(headline["cancelled"], json!(["self__legalApprover"]));
    assert_eq!(headline["superseded"], json!(["self__hrApprover"]));
    for detail in &commit.wfah_entries[2..] {
        let input = detail.input.as_ref().unwrap();
        assert_eq!(input["reason"], json!("sent_back"));
        assert_eq!(input["trigger_kind"], json!("branch"));
    }
    // Ç4-EK: hareket satırı `to_node` taşır — `$valid` eleme kuralı 2 pencereyi
    // bu alandan kurar.
    assert_eq!(
        commit.wfah_entries[0].to_node.as_deref(),
        Some("self__coordinator")
    );

    // Yazıcı ↔ okuyucu el sıkışması: kural 2'nin COLLAPSE dalı tam olarak bu
    // `_collapse` satırını arar ve pencere TÜM kolları (kolsuz satırlar dahil)
    // kapsar. Dal, kol içi geri göndermede tetiklenmez.
    let after = wfes.wfah.extended(&commit.wfah_entries);
    let rules = ValidRules::for_version(&wfd);
    assert_eq!(
        valid::invalid_reason(&after, &rules, &after.entries()[0]),
        Some(valid::InvalidReason::SentBackWindow),
        "geri gönderme penceresindeki satır elenmeli"
    );
}

/// Kural İKİ DALLIDIR: hedef fork alt-grafının İÇİNDEyse collapse YOKTUR — kol
/// hareket eder, paralel mod sürer.
#[tokio::test]
async fn send_back_inside_the_fork_subgraph_stays_a_branch_move() {
    // `self__financeApprover` kardeş kolun giriş node'u → alt-grafın İÇİ.
    let wfd = parallel_with_send_back("self__legalApprover", &["self__financeApprover"]);
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let legal = actor_with_role("legalApprover");
    let wfes = parallel_wfes(
        vec![
            branch("self__financeApprover", BranchStatus::Active, None),
            branch(
                "self__legalApprover",
                BranchStatus::Active,
                Some(legal.user_id),
            ),
        ],
        join_node(),
        parallel_ctx(),
    );

    let commit = engine
        .apply(
            &wfd,
            &wfes,
            &legal,
            "reject",
            &json!({}),
            Some("self__legalApprover"),
            Some("self__financeApprover"),
        )
        .await
        .unwrap();

    assert_eq!(
        commit.outcome,
        CommitOutcome::BranchMoveTo {
            from_node: "self__legalApprover".into(),
            node: "self__financeApprover".into(),
        }
    );
    // Paralel mod sürüyor → hiçbir collapse marker'ı yok.
    assert_eq!(wfah_actions(&commit), vec!["reject"]);
}

/// Ç4-EK/S5 — admin `send_back` paralel modda AÇIK. Acting kol SEÇİLMEZ: tetikleyici
/// `trigger_kind: "admin"` ile yazılır, TÜM kollar düşer, `trigger_actor` gerçek admin.
#[tokio::test]
async fn admin_send_back_in_parallel_collapses_with_admin_trigger() {
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["wf_admin"] = json!([{
        "c_a": {"c_orgu": "self", "c_r": ["coordinator"]},
        "allowed_global_actions": ["send_back"],
    }]);
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let admin = actor_with_role("coordinator");
    let claim_owner = Uuid::new_v4();
    let mut wfes = parallel_wfes(
        vec![
            branch(
                "self__financeApprover",
                BranchStatus::Active,
                Some(claim_owner),
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
            branch("self__hrApprover", BranchStatus::Arrived, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    wfes.origin_orgu_id = Some(admin.orgu_id);
    wfes.visited_nodes = vec!["self__coordinator".into()];

    let commit = engine
        .admin_send_back(&wfd, &wfes, &admin, "self__coordinator", Utc::now())
        .await
        .expect("Ç4-EK/S5: paralel mod kapısı KALKTI");

    let CommitOutcome::CollapseTo {
        from_node,
        node,
        cause,
    } = &commit.outcome
    else {
        panic!("CollapseTo bekleniyordu: {:?}", commit.outcome);
    };
    assert_eq!(*cause, CollapseCause::SentBack);
    // Adminin kolu YOK — "bilinmiyor" değil, "yok".
    assert_eq!(*from_node, None);
    assert_eq!(node, "self__coordinator");

    // Acting kol olmadığı için TÜM kollar düşer (dışlanan kol yok).
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "admin:send_back",
            "_collapse",
            "_branch_cancelled",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let headline = commit.wfah_entries[1].input.clone().unwrap();
    assert_eq!(headline["trigger_kind"], json!("admin"));
    assert!(
        headline["trigger_branch"].is_null(),
        "rastgele bir kol acting SAYILMAZ"
    );
    assert!(headline["trigger_at_node"].is_null());
    assert_eq!(headline["trigger_action"], json!("admin:send_back"));
    assert_eq!(headline["reason"], json!("sent_back"));
    assert_eq!(
        headline["cancelled"],
        json!(["self__financeApprover", "self__legalApprover"])
    );
    assert_eq!(headline["superseded"], json!(["self__hrApprover"]));
    // Trigger AKTÖRÜ gerçek admindir — `system` DEĞİL.
    assert_eq!(headline["trigger_actor"]["user_id"], json!(admin.user_id));
    assert_eq!(commit.wfah_entries[0].actor.user_id, admin.user_id);
}

#[tokio::test]
async fn collapse_marker_carries_dropped_claim_owner() {
    // WOR-59: iptal edilen kolun claim'i adapter'da düşürülür; sahibinin ve
    // claimed_at'in TEK kaydı `_branch_cancelled` marker'ıdır.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["actions"]["finans_ret"]["wft"] = json!({"collapse": {"node": "self__coordinator"}});
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
            "finans_ret",
            &json!({}),
            Some("self__financeApprover"),
            None,
        )
        .await
        .unwrap();

    let cancel = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "_branch_cancelled")
        .expect("_branch_cancelled marker");
    let input = cancel.input.as_ref().unwrap();
    assert_eq!(input["branch_entry"], json!("self__legalApprover"));
    assert_eq!(input["claimed_by"], json!(legal_owner));
    assert_eq!(input["claimed_at"], json!(legal_claimed_at));
}

#[tokio::test]
async fn collapse_summary_marker_describes_whole_event() {
    // WOR-61: collapse'ın tamamı tek `_collapse` kaydından okunabilmeli —
    // tetikleyen kol/aksiyon, hedef, iptal edilen ve geçersizleşen kollar.
    let mut v: Value = serde_json::from_str(PARALLEL_FIXTURE).unwrap();
    v["actions"]["finans_ret"]["wft"] = json!({"collapse": {"node": "self__coordinator"}});
    let wfd = Wfd::from_value(v).unwrap();

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
            "finans_ret",
            &json!({}),
            Some("self__financeApprover"),
            None,
        )
        .await
        .unwrap();

    // manşet DETAY marker'larından önce gelir, detaylar KALIR
    assert_eq!(
        wfah_actions(&commit),
        vec![
            "finans_ret",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded"
        ]
    );
    let summary = &commit.wfah_entries[1];
    assert_eq!(summary.actor.role, "system");
    let input = summary.input.as_ref().unwrap();
    assert_eq!(input["trigger_branch"], json!("self__financeApprover"));
    assert_eq!(input["trigger_action"], json!("finans_ret"));
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
        // Ç3: detay marker'larında da ad `trigger_branch`, değer kol KİMLİĞİ.
        assert_eq!(
            d["trigger_branch"],
            json!("self__financeApprover"),
            "{}",
            detail.action
        );
        assert_eq!(
            d["trigger_action"],
            json!("finans_ret"),
            "{}",
            detail.action
        );
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
        env: Default::default(),
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P1D".into(),
            wfes_effects: None,
            // v2.3 (`Ç9`): hedef yerine GRANT — iş adımda kalır, havuz genişler.
            grant: CaGrantRule {
                c_a: serde_json::from_value(json!({"c_orgu": "self", "c_r": ["branchManager"]}))
                    .unwrap(),
                when: None,
            },
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

    // v2.3 (`Ç1-EK`): kol hareket etmez, yerinde kalır.
    assert!(
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "self__financeApprover"),
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
        env: Default::default(),
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
    // v2.3 (`Ç5`): kimlik ile yönlendirme tek kayıtta — fork aksiyonunun yerine
    // aynı node'dan çıkan bir collapse aksiyonu koyuyoruz.
    v["actions"].as_object_mut().unwrap().remove("start_review");
    v["actions"]["collapse_here"] = json!({
        "label": "X",
        "input": {"required": [], "optional": []},
        "from": "self__coordinator",
        "wft": {"collapse": {"node": "self__requester"}}
    });
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
        env: Default::default(),
    };
    let coord = actor_with_role("coordinator");
    let wfes = wfes_at("self__coordinator", Some(coord.user_id), parallel_ctx());

    let err = engine
        .apply(&wfd, &wfes, &coord, "collapse_here", &json!({}), None, None)
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
    // v2.3 (`Ç5`): iki transition yerine iki AKSİYON kaydı; kıdemli adımın kendi
    // kimliği var (`Ç11`: bir aksiyonu yalnız bir node kullanır).
    v["actions"]["delegate"] = json!({
        "label": "Devret",
        "input": {"required": [], "optional": []},
        "from": "self__financeApprover",
        "wft": {"node": "self__financeSenior"}
    });
    v["actions"]["kidemli_onay"] = json!({
        "label": "Kıdemli Onayı",
        "input": {"required": [], "optional": []},
        "from": "self__financeSenior",
        "wft": {"node": "self__resultCoordinator"}
    });
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
        env: Default::default(),
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
    // finans onayının wft'ini parallel yap (validator dışı, runtime koruması)
    v["actions"]["finans_onay"]["wft"] = json!({
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
        env: Default::default(),
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
            "finans_onay",
            &json!({}),
            Some("self__financeApprover"),
            None,
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
    // v2.3: fork'un `wft`i aksiyon kaydında. Belgeden parallel taşıyan kaydı bul.
    let fork_wft = wfd
        .actions
        .values()
        .find(|a| matches!(a.wft, wfe_core::types::wfd_v22::Wft::Parallel { .. }))
        .expect("paralel fixture bir fork taşımalı")
        .wft
        .clone();
    let start_action = wfd.start[0].action.clone();
    wfd.actions
        .get_mut(&start_action)
        .expect("start aksiyonu")
        .wft = fork_wft;

    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
        env: Default::default(),
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .claim_timeout = Some(ClaimTimeout {
        after: "PT2H".into(),
        wfes_effects: None,
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
                "claim_released:self__financeApprover"
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
        env: Default::default(),
    };
    let mut wfd = paralel();
    wfd.nodes
        .get_mut("self__financeApprover")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P1D".into(),
            wfes_effects: None,
            // v2.3 (`Ç9`): hedef yerine GRANT — iş adımda kalır, havuz genişler.
            grant: CaGrantRule {
                c_a: serde_json::from_value(json!({"c_orgu": "self", "c_r": ["branchManager"]}))
                    .unwrap(),
                when: None,
            },
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

    // v2.3 (`Ç1-EK`): kademe kolu da HAREKET ETTİRMEZ. Eskiden escalation `wft`i
    // join'i hedefleyince kol VARIŞ sayılıyordu; artık kol yerinde kalır, yalnız
    // yetki havuzu genişler — dolayısıyla `_branch_arrived` marker'ı da YAZILMAZ.
    let commit = engine
        .fire_escalation(&wfd, &wfes, 0, now, Some("self__financeApprover"))
        .await
        .unwrap();
    assert!(
        matches!(&commit.outcome, CommitOutcome::StayAt { node } if node == "self__financeApprover"),
        "{:?}",
        commit.outcome
    );
    assert_eq!(
        wfah_actions(&commit),
        vec!["escalate:self__financeApprover:0"]
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
        env: Default::default(),
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
        .map(|e| &e.input.as_ref().unwrap()["branch_entry"])
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
        c_orgu: Some(COrgu::Selector("self".into())),
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
        env: Default::default(),
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu); // mevcut sahip
    let mgr = manager(orgu); // amir (reassign kuralına uyar)
    let target = analyst(orgu); // node c_a'ya uygun yeni sahip
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &mgr, Some(&target), None, Utc::now())
        .await
        .expect("yetkili amir uygun hedefe devredebilmeli");

    // Ç13/E12/S4: kişiden kişiye devir İKİ satır — önce eski sahibin bırakması,
    // sonra yeni sahibin alması. Ardışık `seq`, aynı transaction.
    assert_eq!(entries.len(), 2, "kişiden kişiye devir İKİ satır yazar");
    assert_eq!(entries[1].seq, entries[0].seq + 1, "seq ardışık olmalı");
    for e in &entries {
        assert_eq!(e.actor.user_id, mgr.user_id, "satır aktörü amir olmalı");
        assert!(e.from_node.is_none() && e.to_node.is_none(), "marker satırı");
    }

    let released = &entries[0];
    assert_eq!(released.action, "claim_released:self__creditAnalyst");
    let input = released.input.as_ref().unwrap();
    assert_eq!(input["reason"], json!("taken_by_other"));
    assert_eq!(
        input["owner"],
        json!(owner.user_id.to_string()),
        "bırakma satırının öznesi ESKİ sahiptir"
    );
    assert!(input.get("authority").is_none(), "authority yalnız claim_taken'da");

    let taken = &entries[1];
    assert_eq!(taken.action, "claim_taken:self__creditAnalyst");
    let input = taken.input.as_ref().unwrap();
    assert_eq!(input["via"], json!("assigned"));
    assert_eq!(input["authority"], json!("c_a"));
    assert_eq!(
        input["owner"],
        json!(target.user_id.to_string()),
        "alma satırının öznesi YENİ sahiptir"
    );
    assert_eq!(
        input["waited_for_seconds"],
        json!(0),
        "yetkili devirde bekleme YOKTUR (E12/S2)"
    );
    // Eski `{from, to}` şekli KALKTI — bir satır = bir sahiplik öznesi.
    assert!(input.get("from").is_none() && input.get("to").is_none());
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
        env: Default::default(),
    };
    let wfd = golden_with_reassign();

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let mgr = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &mgr, None, None, Utc::now())
        .await
        .expect("amir havuza bırakabilmeli");

    // E12/S4: havuza bırakmada alan yoktur → TEK satır.
    assert_eq!(entries.len(), 1, "havuza bırakma TEK satır yazar");
    assert_eq!(entries[0].action, "claim_released:self__creditAnalyst");
    let input = entries[0].input.as_ref().unwrap();
    assert_eq!(input["reason"], json!("taken_by_other"));
    assert_eq!(input["owner"], json!(owner.user_id.to_string()));
}

/// Sahip işi KENDİ bırakırsa sebep `self`tir — "yetkili başkası aldı" ile aynı
/// satıra düşerse denetimde iki farklı olay ayırt edilemez.
#[tokio::test]
async fn owner_releasing_their_own_claim_is_reason_self() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let mut wfd = golden_with_reassign();
    // Sahibin kendi bırakabilmesi için devir kuralı onun rolünü de kapsamalı.
    wfd.nodes.get_mut("self__creditAnalyst").unwrap().reassign = Some(CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: Some(vec!["creditAnalyst".into()]),
        c_u: None,
    });

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &owner, None, None, Utc::now())
        .await
        .expect("sahip kendi bırakabilmeli");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].input.as_ref().unwrap()["reason"],
        json!("self")
    );
}

/// E12/S2: havuzdan atamada bekleme GERÇEKTİR ve tabanı `node/kol girişi` ile
/// `son bırakma anı`ndan YENİ olanıdır — al-bırak-al döngüsünde ikinci sahibin
/// beklemesi birincinin tutma süresini kapsamaz.
#[tokio::test]
async fn assign_from_pool_waits_from_the_last_release_not_node_entry() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::AssignFromPool]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let target = analyst(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    let entered_at = wfes.wfah.entries()[0].applied_at;
    // Node girişinden 1 saat sonra bir bırakma satırı düşmüş olsun.
    wfes.wfah.0.push(WfahEntry {
        seq: 2,
        action: "claim_released:self__creditAnalyst".into(),
        actor: analyst(orgu),
        input: Some(json!({"reason": "self"})),
        applied_at: entered_at + Duration::hours(1),
        from_node: None,
        to_node: None,
        branch_entry: None,
        branch_round: None,
    });

    let now = entered_at + Duration::hours(3);
    let entries = engine
        .reassign(&wfd, &wfes, &admin, Some(&target), None, now)
        .await
        .expect("havuzdan atama");
    assert_eq!(
        entries[0].input.as_ref().unwrap()["waited_for_seconds"],
        json!(2 * 3600),
        "taban SON BIRAKMA anıdır (3sa − 1sa), node girişi değil"
    );
}

/// Değişmez #2'nin bu karardaki hâli: eski sahiplik adları motordan KALKTI.
/// `parse_marker` onları hâlâ `Action`a düşürür (eski satırlar dönüştürülmüyor),
/// ama motor bir daha ÜRETMEZ — `Action` sınıfı bu kadar daraldı.
#[tokio::test]
async fn engine_no_longer_emits_the_old_ownership_action_names() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let target = analyst(orgu);
    let claimed = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());
    let pooled = wfes_at("self__creditAnalyst", None, start_input());

    let mut produced: Vec<String> = Vec::new();
    for (wfes, target) in [
        (&claimed, Some(&target)),
        (&claimed, None),
        (&pooled, Some(&target)),
    ] {
        let entries = engine
            .reassign(&wfd, wfes, &admin, target, None, Utc::now())
            .await
            .expect("wf_admin her üç yolu da alabilir");
        produced.extend(entries.iter().map(|e| e.action.clone()));
    }
    for action in &produced {
        assert!(
            action.starts_with("claim_taken:") || action.starts_with("claim_released:"),
            "eski ad üretildi: {action}"
        );
        assert!(
            !parse_marker(action).kind.is_action(),
            "sahiplik satırı `Action`a düşmemeli: {action}"
        );
    }
    assert_eq!(produced.len(), 4, "2 + 1 + 1 satır: {produced:?}");
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
        env: Default::default(),
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
    assert_eq!(
        new.initial_dynctx["applicant"]["name"],
        json!("Ayşe Yılmaz")
    );
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
        env: Default::default(),
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
    // v2.3 (`Ç11`): onay aksiyonunun adı kol başına ayrıdır — bir aksiyonu yalnız
    // bir node kullanır, dolayısıyla üç kol `approve` adını paylaşamaz.
    let action = match node {
        "self__financeApprover" => "finans_onay",
        "self__legalApprover" => "hukuk_onay",
        "self__hrApprover" => "ik_onay",
        other => panic!("bilinmeyen kol node'u: {other}"),
    };
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    engine
        .apply(
            &paralel(),
            wfes,
            actor,
            action,
            &json!({}),
            Some(node),
            None,
        )
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
            assert!(
                matches!(*next, CommitOutcome::MoveTo { ref node } if node == "self__resultCoordinator")
            );
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

// ================================================================ T‑A5: WF Admin
// Akış-içi yetkili: node'un kendi `reassign` kuralı olmasa da devredebilir, escalation
// sayacına müdahale edebilir. Tasarım: docs/superpowers/specs/2026-08-11-wf-admin-design.md

/// Golden'a WF Admin kuralı ekler: bu akışta branchManager akış yöneticisidir.
fn golden_with_wf_admin(when: Option<&str>) -> Wfd {
    let mut wfd = golden();
    wfd.wf_admin = vec![WfAdminRule {
        grant: CaGrantRule {
            c_a: CandidateActor {
                c_orgu: Some(COrgu::Selector("self".into())),
                c_r: Some(vec!["branchManager".into()]),
                c_u: None,
            },
            when: when.map(String::from),
        },
        // A-1: yetki artık listeden gelir. Bu fixture "tam yetkili" admini kurar;
        // yetkisiz admin senaryoları listeyi DARALTARAK test edilir.
        allowed_global_actions: vec![
            GlobalAction::AssignFromPool,
            GlobalAction::ReclaimToPool,
            GlobalAction::Reassign,
            GlobalAction::SendBack,
            GlobalAction::SendToStart,
            GlobalAction::Cancel,
            GlobalAction::FireEscalation,
            GlobalAction::SkipEscalation,
        ],
    }];
    wfd
}

fn test_engine<'a>(org: &'a MockOrg, runner: &'a MockRunner) -> Engine<'a> {
    Engine {
        org,
        exec: runner,
        env: Default::default(),
    }
}

/// WF Admin, node'un `reassign` kuralı OLMASA da devredebilir — "tek yerde ayarla".
#[tokio::test]
async fn wf_admin_can_reassign_on_node_without_reassign_rule() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);
    assert!(
        wfd.nodes["self__creditAnalyst"].reassign.is_none(),
        "test öncülü: node'un kendi reassign kuralı olmamalı"
    );

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let target = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &admin, Some(&target), None, Utc::now())
        .await
        .expect("wf_admin devredebilmeli");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].action, "claim_released:self__creditAnalyst");
    assert_eq!(entries[1].action, "claim_taken:self__creditAnalyst");
    assert_eq!(entries[1].actor.user_id, admin.user_id);
}

/// Satır hangi yoldan geldiğini söyler: denetimde "node amiri mi, akış admini mi"
/// ayrımı gerekir. Ç13: ayrım artık `via`/`reason` KAPALI LİSTESİNDEDİR — eski
/// `via: "wf_admin"` serbest metniyle karışmasın.
#[tokio::test]
async fn wf_admin_reassign_marker_records_via() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &admin, None, None, Utc::now())
        .await
        .expect("havuza bırakabilmeli");
    let input = entries[0].input.as_ref().unwrap();
    assert_eq!(input["reason"], json!("admin"));
    assert_eq!(input["global_action"], json!("reclaim_to_pool"));
}

/// Node'un kendi kuralıyla gelen devir WF Admin yolundan AYIRT EDİLİR: `reason`
/// farklıdır ve `global_action` YAZILMAZ (o alan yalnız admin kapısının izidir).
#[tokio::test]
async fn node_reassign_path_is_not_an_admin_release() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_reassign(); // wf_admin YOK

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let mgr = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &mgr, None, None, Utc::now())
        .await
        .expect("node amiri devredebilmeli");
    let input = entries[0].input.as_ref().unwrap();
    assert_eq!(input["reason"], json!("taken_by_other"));
    assert!(
        input.get("global_action").is_none(),
        "node.reassign yolu global aksiyon kapısından geçmez: {input:?}"
    );
}

/// WF Admin de hedefi node'un c_a'sına uymaya zorlar: uymayan hedef claim'i tutar ama
/// hiçbir aksiyon alamaz — WF Admin akışı kilitlemiş olurdu.
#[tokio::test]
async fn wf_admin_reassign_still_requires_eligible_target() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let bad_target = manager(orgu); // creditAnalyst node'unun c_a'sına uymaz

    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());
    let err = engine
        .reassign(&wfd, &wfes, &admin, Some(&bad_target), None, Utc::now())
        .await
        .expect_err("uygunsuz hedef reddedilmeli");
    assert!(matches!(err, EngineError::TargetNotEligible), "{err:?}");
}

/// Kural eşleşmiyorsa yetki yok (kapı `wf_admin` VARLIĞIYLA açılmaz).
#[tokio::test]
async fn non_matching_wf_admin_rule_does_not_authorize_reassign() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let mut wfd = golden_with_wf_admin(None);
    wfd.wf_admin[0].grant.c_a.c_r = Some(vec!["auditor".into()]); // kimse bu rolde değil

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let err = engine
        .reassign(&wfd, &wfes, &admin, None, None, Utc::now())
        .await
        .expect_err("eşleşmeyen kural yetki vermemeli");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// `when` guard'ı false ise yetki yok.
#[tokio::test]
async fn wf_admin_when_guard_false_does_not_authorize() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(Some("false"));

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let err = engine
        .reassign(&wfd, &wfes, &admin, None, None, Utc::now())
        .await
        .expect_err("when false iken yetki olmamalı");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

// ---- escalation müdahalesi ----

/// Atlama marker'ı `:skipped` sonekiyle yazılır ve geçiş UYGULANMAZ.
#[tokio::test]
async fn skip_escalation_writes_skipped_marker() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let skip = engine
        .skip_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect("atlama çalışmalı")
        .expect("bekleyen adım olmalı");
    assert_eq!(skip.step_idx, 0);
    assert_eq!(skip.node, "self__creditAnalyst");
    assert_eq!(skip.marker, "escalate:self__creditAnalyst:0:skipped");
    assert_eq!(skip.entry.action, skip.marker);
    assert_eq!(
        skip.entry.actor.user_id, admin.user_id,
        "iz admini gösterir"
    );
}

/// Atlanan adım bir daha ateşlenmez.
#[tokio::test]
async fn skipped_escalation_step_does_not_refire() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    let skip = engine
        .skip_escalation(&wfd, &wfes, &admin, None, entered_at)
        .await
        .unwrap()
        .unwrap();
    wfes.wfah = Wfah(
        wfes.wfah
            .entries()
            .iter()
            .cloned()
            .chain(std::iter::once(skip.entry))
            .collect(),
    );

    let now = entered_at + Duration::days(10);
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, now, None).unwrap(),
        None,
        "atlanan adım tekrar due olmamalı"
    );
}

/// KRİTİK: atlama marker'ı escalation TABANINI kaydırmaz. `next_escalation` node giriş
/// zamanını "son escalation-DIŞI kayıt"tan hesaplıyor; marker `escalate:` önekini
/// taşımasa tüm sayaçlar sessizce sıfırlanırdı.
#[tokio::test]
async fn skipping_does_not_shift_the_escalation_base() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let mut wfd = golden_with_wf_admin(None);
    // İkinci adım (P5D) ekle: atlama sonrası ONUN vadesi kaymamalı.
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .escalation
        .push(EscalationStep {
            after: "P5D".into(),
            wfes_effects: None,
            // v2.3 (`Ç9`): hedef yerine GRANT — iş adımda kalır, havuz genişler.
            grant: CaGrantRule {
                c_a: serde_json::from_value(json!({"c_orgu": "self", "c_r": ["branchManager"]}))
                    .unwrap(),
                when: None,
            },
        });

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    let entered_at = wfes.wfah.entries().last().unwrap().applied_at;

    let before = engine
        .next_escalation(&wfd, &wfes, entered_at, None)
        .unwrap()
        .unwrap();
    assert_eq!(before.step_idx, 0);

    // Atlama 4 gün SONRA yapılıyor — taban kaysa adım 1'in vadesi +9 güne giderdi.
    let skip_at = entered_at + Duration::days(4);
    let skip = engine
        .skip_escalation(&wfd, &wfes, &admin, None, skip_at)
        .await
        .unwrap()
        .unwrap();
    wfes.wfah = Wfah(
        wfes.wfah
            .entries()
            .iter()
            .cloned()
            .chain(std::iter::once(skip.entry))
            .collect(),
    );

    let after = engine
        .next_escalation(&wfd, &wfes, skip_at, None)
        .unwrap()
        .expect("adım 1 hâlâ beklemede olmalı");
    assert_eq!(after.step_idx, 1);
    assert_eq!(
        after.entered_at, before.entered_at,
        "atlama node giriş zamanını DEĞİŞTİRMEMELİ"
    );
    assert_eq!(
        after.deadline,
        before.entered_at + Duration::days(5),
        "adım 1'in vadesi node girişinden ölçülmeye devam etmeli"
    );
}

// ------------------------------------------ R02: escalation tabanı = hareket satırı

/// R02 yardımcısı: HAREKET satırı (`to_node` dolu) + ardından marker satırları
/// (`to_node` NULL) taşıyan kontrollü bir defter kurar.
fn wfah_with_markers(
    node: &str,
    entered_at: chrono::DateTime<Utc>,
    markers: &[(&str, chrono::DateTime<Utc>)],
) -> Wfah {
    let system = Actor {
        orgu_id: Uuid::nil(),
        user_id: Uuid::nil(),
        role: "system".into(),
    };
    let mut entries = vec![WfahEntry {
        seq: 1,
        action: "start".into(),
        actor: system.clone(),
        input: None,
        applied_at: entered_at,
        from_node: None,
        to_node: Some(node.into()),
        branch_entry: None,
        branch_round: None,
    }];
    for (i, (action, at)) in markers.iter().enumerate() {
        entries.push(WfahEntry {
            seq: (i + 2) as u32,
            action: (*action).into(),
            actor: system.clone(),
            input: None,
            applied_at: *at,
            from_node: None,
            to_node: None,
            branch_entry: None,
            branch_round: None,
        });
    }
    Wfah(entries)
}

/// R02/S1: HİÇBİR marker türü escalation tabanını kaydırmaz — soru ada değil
/// `to_node`a bakıyor. Eski önek filtresi (`!starts_with("escalate:")`) bu satırların
/// HEPSİNİ tabana geçiriyordu; her yeni marker türü sessiz bir kayma demekti.
#[tokio::test]
async fn marker_rows_never_shift_the_escalation_base() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden();

    let t0 = Utc::now();
    // Son eleman BİLEREK tanınmayan bir ad: kapı bir ad listesine değil `to_node`a
    // dayandığı için yarın eklenecek marker türü de kendiliğinden dışarıda kalır.
    let markers = [
        ("trigger:use_scoring", t0 + Duration::hours(1)),
        ("call:sub/gonder", t0 + Duration::hours(2)),
        ("_branch_arrived", t0 + Duration::hours(3)),
        ("_collapse", t0 + Duration::hours(4)),
        ("_join", t0 + Duration::hours(5)),
        ("claim_released:self__creditAnalyst", t0 + Duration::hours(6)),
        ("claim_taken:self__creditAnalyst", t0 + Duration::hours(7)),
        ("_marker_that_does_not_exist_yet", t0 + Duration::hours(8)),
    ];

    for n in 1..=markers.len() {
        let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
        wfes.wfah = wfah_with_markers("self__creditAnalyst", t0, &markers[..n]);
        let forecast = engine
            .next_escalation(&wfd, &wfes, t0, None)
            .unwrap()
            .expect("adım 0 beklemede olmalı");
        assert_eq!(
            forecast.entered_at, t0,
            "'{}' satırı tabanı kaydırmamalı",
            markers[n - 1].0
        );
        assert_eq!(
            forecast.deadline,
            t0 + Duration::days(3),
            "vade node girişinden (P3D) ölçülmeli — '{}' sonrası da",
            markers[n - 1].0
        );
    }
}

/// R02, Ç1 gerileme kapısı: `escalate:` satırı tabanı kaydırmaz. Eskiden bunu ÖNEK
/// sağlıyordu; artık `to_node`un NULL olması sağlıyor ve ada hiç bakılmıyor.
#[tokio::test]
async fn an_escalation_marker_row_does_not_shift_the_base() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
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
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.wfah = wfah_with_markers(
        "self__creditAnalyst",
        t0,
        &[("escalate:self__creditAnalyst:0", t0 + Duration::days(3))],
    );

    let forecast = engine
        .next_escalation(&wfd, &wfes, t0 + Duration::days(3), None)
        .unwrap()
        .expect("adım 1 beklemede olmalı");
    assert_eq!(forecast.step_idx, 1);
    assert_eq!(forecast.entered_at, t0, "taban node girişinde kalmalı");
    assert_eq!(
        forecast.deadline,
        t0 + Duration::days(5),
        "adım 1'in vadesi node girişinden ölçülmeli, marker'dan değil"
    );
}

/// R02, "bedava gelen düzelme": ateşlenmiş kademe SONRASINDA bir marker satırı gelse
/// bile kademe HÂLÂ `settled` — TEKRAR ATEŞLEME YOLU KAPALI.
///
/// Eski tabanla marker satırı tabanı kendi anına atıyordu; `escalate:…:0` satırı o yeni
/// tabandan ÖNCE kaldığı için `applied_at >= entered_at` düşüyor, ateşlenmiş kademe
/// "ateşlenmemiş" sayılıyor ve `due_escalation` onu ikinci kez veriyordu.
#[tokio::test]
async fn a_fired_step_stays_settled_after_a_later_marker_row() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden();

    let t0 = Utc::now();
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.wfah = wfah_with_markers(
        "self__creditAnalyst",
        t0,
        &[
            // Tek kademe (P3D) ateşlendi…
            ("escalate:self__creditAnalyst:0", t0 + Duration::days(3)),
            // …sonra ALAKASIZ bir marker satırı düştü.
            ("trigger:use_scoring", t0 + Duration::days(4)),
        ],
    );

    let now = t0 + Duration::days(10);
    assert_eq!(
        engine.next_escalation(&wfd, &wfes, now, None).unwrap(),
        None,
        "ateşlenmiş tek kademe sonrası bekleyen adım OLMAMALI"
    );
    assert_eq!(
        engine.due_escalation(&wfd, &wfes, now, None).unwrap(),
        None,
        "ateşlenmiş kademe marker satırından sonra TEKRAR due olmamalı"
    );
}

/// R02/S2: migration öncesi satırlarda `to_node` NULL'dır ve bu iş onlar için YEDEK YOL
/// KURMAZ — hareket taşıyan satır yoksa cevap `None`. Eski WFE'lerde escalation susar;
/// NULL'ların ne olacağı R01'in işi.
#[tokio::test]
async fn no_movement_row_means_no_forecast() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden();

    let t0 = Utc::now();
    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    // Defterin TAMAMI `to_node` NULL — migration öncesi satırların hâli.
    let mut wfah = wfah_with_markers("self__creditAnalyst", t0, &[("start_review", t0)]);
    wfah.0[0].to_node = None;
    wfes.wfah = wfah;

    assert_eq!(
        engine
            .next_escalation(&wfd, &wfes, t0 + Duration::days(30), None)
            .unwrap(),
        None,
        "hareket taşıyan satır yoksa forecast üretilmez (fallback YOK)"
    );
    assert_eq!(
        engine
            .due_escalation(&wfd, &wfes, t0 + Duration::days(30), None)
            .unwrap(),
        None,
        "taban bulunamayan WFE'de escalation ATEŞLENMEZ"
    );
}

/// Escalation müdahalesi `node.reassign` ile AÇILMAZ — farklı bir güç.
#[tokio::test]
async fn skip_escalation_requires_wf_admin() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_reassign(); // node amiri var, wf_admin YOK

    let orgu = Uuid::new_v4();
    let mgr = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let err = engine
        .skip_escalation(&wfd, &wfes, &mgr, None, Utc::now())
        .await
        .expect_err("node amiri sayacı yönetemez");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Bekleyen adım yoksa cevap `None` — hata değil (route bunu 409'a çevirir).
#[tokio::test]
async fn skip_escalation_returns_none_when_no_step_pending() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let mut wfd = golden_with_wf_admin(None);
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .escalation
        .clear();

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    assert!(engine
        .skip_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .unwrap()
        .is_none());
}

/// Elle tetikleme OTOMATİK yolun aynı marker'ını yazar (yayınlanmış akışların
/// `count($wfah, ...)` sayımları bozulmasın); ayrım AKTÖRDEDİR.
#[tokio::test]
async fn manual_fire_uses_same_marker_but_records_admin_actor() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    // Vade GELMEDEN tetikleniyor — erken tetikleme bu ucun varlık sebebi.
    let commit = engine
        .fire_escalation_by(&wfd, &wfes, 0, Utc::now(), None, &admin)
        .await
        .expect("elle tetikleme çalışmalı");
    let marker = commit
        .wfah_entries
        .iter()
        .find(|e| e.action.starts_with("escalate:"))
        .expect("escalation marker'ı yazılmalı");
    assert_eq!(marker.action, "escalate:self__creditAnalyst:0");
    assert_eq!(marker.actor.user_id, admin.user_id);
}

/// Elle tetiklemenin YETKİ KAPISI çekirdektedir: executor'da unutulabilecek bir
/// denetim, yetkisiz bir aktörün akışı ilerletmesi demek olurdu.
#[tokio::test]
async fn admin_fire_escalation_requires_wf_admin() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_reassign(); // node amiri var, wf_admin YOK

    let mgr = manager(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let err = engine
        .admin_fire_escalation(&wfd, &wfes, &mgr, None, Utc::now())
        .await
        .expect_err("wf_admin olmadan tetiklenemez");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Yetkili aktör vade gelmeden tetikleyebilir; sonuç adım index'i + commit'tir.
#[tokio::test]
async fn admin_fire_escalation_applies_step_before_deadline() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let admin = manager(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    let (step_idx, commit) = engine
        .admin_fire_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect("tetikleme çalışmalı")
        .expect("bekleyen adım olmalı");
    assert_eq!(step_idx, 0);
    assert!(commit
        .wfah_entries
        .iter()
        .any(|e| e.action == "escalate:self__creditAnalyst:0"));
}

/// Bekleyen adım yoksa `None` (rota 409'a çevirir).
#[tokio::test]
async fn admin_fire_escalation_returns_none_when_nothing_pending() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let mut wfd = golden_with_wf_admin(None);
    wfd.nodes
        .get_mut("self__creditAnalyst")
        .unwrap()
        .escalation
        .clear();

    let admin = manager(Uuid::new_v4());
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    assert!(engine
        .admin_fire_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .unwrap()
        .is_none());
}

/// Spec §6.1/10 — BİTMİŞ akışın sayacı yönetilmez (iki uç da reddeder).
#[tokio::test]
async fn escalation_admin_endpoints_reject_terminal_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);
    let admin = manager(Uuid::new_v4());

    let mut wfes = wfes_at("self__creditAnalyst", None, start_input());
    wfes.status = WfeStatus::Terminal;

    let err = engine
        .skip_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect_err("biten akışta atlama reddedilmeli");
    assert!(matches!(err, EngineError::WfeTerminal), "{err:?}");

    let err = engine
        .admin_fire_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect_err("biten akışta tetikleme reddedilmeli");
    assert!(matches!(err, EngineError::WfeTerminal), "{err:?}");
}

/// WF Admin olmak AKSİYON yetkisi VERMEZ — tasarımın özü: işi yönetir, işi yapmaz.
///
/// `wf_admin` kuralına uyan ama node'un `c_a`'sına uymayan aktör `apply_action`'dan
/// geçmemeli; geçseydi WF Admin, tasarımcının hiçbir node'unda öngörmediği bir ACT
/// kanalı kazanırdı.
#[tokio::test]
async fn wf_admin_does_not_grant_action_rights() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None);

    let orgu = Uuid::new_v4();
    // Admin (branchManager) creditAnalyst node'unun c_a'sına UYMAZ.
    let admin = manager(orgu);
    let wfes = wfes_at("self__creditAnalyst", None, start_input());

    // Yetki kanalı CLAIM kapısında yalıtılır: `apply` önce atama ister (NotClaimed),
    // o yüzden c_a denetimini doğrudan sınamak için claim kullanılır.
    assert_eq!(
        engine.can_claim(&wfd, &wfes, &admin, None).await.unwrap(),
        ClaimCheck::NotEligible,
        "wf_admin, node c_a'sına uymadığı halde claim edebiliyor"
    );

    // Karşı kanıt: node c_a'sına UYAN aktör claim edebilir — yani yukarıdaki red
    // gerçekten YETKİDEN geliyor, node'un genel bir kilidinden değil.
    let analyst = analyst(orgu);
    assert_eq!(
        engine.can_claim(&wfd, &wfes, &analyst, None).await.unwrap(),
        ClaimCheck::Ok
    );
}

// ---- Girdi TİP kapısı (2026-08-19) ----
//
// Motor bilir kişidir: bildirilen bir tip varsa ve değer o tipte gelmiyorsa reddi
// ENGINE verir. İstemci (editör, portal, üçüncü parti UI) kendi tip kuralını icat
// etmez — kapı burada. Denetim çekirdeği `wfe_core::v22::ctx_types`.

/// Golden'da `credit_info.amount_requested` `number`; METİN gönderilirse REDDEDİLİR.
/// (2026-08-19 öncesi bu değer sessizce ctx'e yazılıyordu ve etkisi ancak sayısal bir
/// `when` çalışırken çıkıyordu.)
#[tokio::test]
async fn wrong_typed_start_input_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["credit_info"]["amount_requested"] = json!("yüz bin");

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
    match &err {
        EngineError::InputTypeMismatch(violations) => {
            assert_eq!(violations.len(), 1, "{violations:?}");
            assert_eq!(violations[0].path, "credit_info.amount_requested");
            assert!(
                violations[0].expected.contains("number"),
                "{:?}",
                violations[0]
            );
            assert!(violations[0].got.contains("string"), "{:?}", violations[0]);
        }
        other => panic!("tip ihlali beklendi: {other}"),
    }
}

/// Doğru tip geçer — kapı meşru girdiyi engellemez.
#[tokio::test]
async fn correctly_typed_start_input_passes() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let actor = clerk(Uuid::new_v4());
    engine
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
        .expect("doğru tipli girdi geçmeli");
}

/// Aksiyon (apply) yolu da AYNI kapıdan geçer: `manager_decision` enum'dur, listede
/// olmayan değer reddedilir.
#[tokio::test]
async fn enum_violation_on_apply_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let manager = actor_with_role("branchManager");
    let wfes = wfes_at("self__branchManager", Some(manager.user_id), json!({}));

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &manager,
            "manager_decide",
            &json!({ "manager_decision": "belki" }),
            None,
            None,
        )
        .await
        .unwrap_err();
    match &err {
        EngineError::InputTypeMismatch(v) => {
            assert_eq!(v[0].path, "manager_decision");
            assert!(v[0].expected.contains("approve"), "{:?}", v[0]);
        }
        other => panic!("enum ihlali beklendi: {other}"),
    }
}

/// TİP denetimi bildirim denetiminden SONRA koşar: tanımsız yola gönderilen bozuk
/// değerde kullanıcı asıl sorunu ("yol bildirilmemiş") görmeli.
#[tokio::test]
async fn undeclared_path_error_wins_over_type_error() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let actor = clerk(Uuid::new_v4());
    let mut input = start_input();
    input["credit_score"] = json!("metin"); // hem tanımsız hem yanlış tip

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

// ---- Kapı B: ctx'e YAZILAN değerin tip denetimi (2026-08-19) ----
//
// Kapı A isteğin girdisini görür; kapı B `wfes_effects` ile bağlama yazılan HER şeyi
// (autoexec sonucu · WFC dönüşü · `$env` · sistem/sabit) tek noktada denetler. Yalnız
// DEĞİŞEN kök alanlar sorulur: enforcement öncesi bozulmuş eski veri akışı durdurmaz.

/// Autoexec `credit_score`a METİN yazarsa (şemada `number`) geçiş UYGULANMAZ.
/// Bu, kapı A'nın göremediği sınıftır: değer istekten değil DIŞ SİSTEMDEN geliyor.
#[tokio::test]
async fn autoexec_result_with_wrong_type_is_rejected() {
    let org = MockOrg {
        role_assigned: true,
    };
    // `MockRunner::ok` skoru `credit_score`a yazıyor; burada METİN döndüren bir runner.
    let runner = MockRunner::with_rest_result(json!({ "score": "yüksek", "grade": "A" }), true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let analyst = actor_with_role("creditAnalyst");
    let wfes = wfes_at("self__creditAnalyst", Some(analyst.user_id), json!({}));

    let err = engine
        .apply(
            &golden(),
            &wfes,
            &analyst,
            "analyst_approve",
            &json!({ "credit_info": { "amount_requested": 1000 } }),
            None,
            None,
        )
        .await
        .unwrap_err();
    match &err {
        EngineError::CtxTypeMismatch(v) => {
            assert!(
                v.iter().any(|x| x.path == "credit_score"),
                "ihlal `credit_score`u göstermeli: {v:?}"
            );
        }
        other => panic!("ctx tip ihlali beklendi: {other}"),
    }
}

/// Doğru tipli autoexec sonucu geçer — kapı meşru akışı engellemez.
#[tokio::test]
async fn correctly_typed_autoexec_result_passes() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let analyst = actor_with_role("creditAnalyst");
    let wfes = wfes_at("self__creditAnalyst", Some(analyst.user_id), json!({}));

    let commit = engine
        .apply(
            &golden(),
            &wfes,
            &analyst,
            "analyst_approve",
            &json!({ "credit_info": { "amount_requested": 1000 } }),
            None,
            None,
        )
        .await
        .expect("doğru tipli sonuç geçmeli");
    assert_eq!(commit.new_dynctx["credit_score"], json!(750));
}

/// ÖNCEDEN bozulmuş bir alan (bu geçişte YAZILMAYAN) akışı durdurmaz: kapı B yalnız
/// bu commit'in yazdığına bakar. Bozuk veriyle iş yapmayı engellemek ayrı bir kapının
/// (`ctx_types::validate_dynctx` → "kapı C") işidir.
#[tokio::test]
async fn preexisting_corrupt_field_does_not_block_the_transition() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let analyst = actor_with_role("creditAnalyst");
    // `internal_notes` şemada `string`; DB'de sayı olarak duruyor (enforcement öncesi).
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst.user_id),
        json!({ "internal_notes": 42 }),
    );

    engine
        .apply(
            &golden(),
            &wfes,
            &analyst,
            "analyst_approve",
            &json!({ "credit_info": { "amount_requested": 1000 } }),
            None,
            None,
        )
        .await
        .expect("bu geçişte yazılmayan bozuk alan akışı durdurmamalı");
}

// ============================================== GLOBAL AKSİYONLAR (A-2, 2026-08-21)
//
// Yetki artık ÖRTÜK DEĞİL: `wf_admin` kuralına uymak yalnız görme verir, her müdahale
// `allowed_global_actions`ta yazmak zorunda. Aşağıdaki testler kapıyı (kim), süzgeci
// (nereye) ve denetim izini (kim yaptı) ayrı ayrı sınar.

/// `golden_with_wf_admin`in yetki-daraltılmış hâli: admin YALNIZ verilen aksiyonları
/// alabilir. "Tam yetkili" fixture kapının kapalı hâlini test edemez.
fn golden_with_admin_actions(actions: &[GlobalAction]) -> Wfd {
    let mut wfd = golden_with_wf_admin(None);
    wfd.wf_admin[0].allowed_global_actions = actions.to_vec();
    wfd
}

/// Boş liste = HİÇBİR müdahale (güvenli varsayılan). Kurala uymak yetmez.
#[tokio::test]
async fn empty_allowed_global_actions_grants_nothing() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    assert!(
        engine
            .admin_global_actions(&wfd, &wfes, &admin)
            .await
            .unwrap()
            .is_empty(),
        "boş liste hiçbir aksiyon vermemeli"
    );
    // Eskiden bu çağrı GEÇİYORDU (kurala uymak devri açardı) — kırılma bilinçli.
    let err = engine
        .reassign(&wfd, &wfes, &admin, None, None, Utc::now())
        .await
        .expect_err("yetkisiz admin devredemez");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Devrin ÜÇ hâli ÜÇ ayrı yetkidir: yalnız `reclaim_to_pool` verilen admin işi havuza
/// alabilir ama kimseye ATAYAMAZ. Hassas akışta istenen ayrım tam olarak budur.
#[tokio::test]
async fn reclaim_to_pool_does_not_grant_assign() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::ReclaimToPool]);

    let orgu = Uuid::new_v4();
    let owner = analyst(orgu);
    let admin = manager(orgu);
    let target = analyst(orgu);
    let wfes = wfes_at("self__creditAnalyst", Some(owner.user_id), start_input());

    let entries = engine
        .reassign(&wfd, &wfes, &admin, None, None, Utc::now())
        .await
        .expect("havuza alma yetkisi var");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, "claim_released:self__creditAnalyst");
    assert_eq!(
        entries[0].input.as_ref().unwrap()["global_action"],
        json!("reclaim_to_pool")
    );

    let err = engine
        .reassign(&wfd, &wfes, &admin, Some(&target), None, Utc::now())
        .await
        .expect_err("atama yetkisi YOK");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Havuzdaki (sahipsiz) işi kişiye atamak `assign_from_pool`, kişiden kişiye devir
/// `reassign` — ikisi ayrı yetkidir ve denetim izi hangisi olduğunu söyler.
#[tokio::test]
async fn assign_from_pool_and_reassign_are_separate_powers() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::AssignFromPool]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let target = analyst(orgu);
    // Sahipsiz (havuzda) iş → assign_from_pool
    let unclaimed = wfes_at("self__creditAnalyst", None, start_input());
    let entries = engine
        .reassign(&wfd, &unclaimed, &admin, Some(&target), None, Utc::now())
        .await
        .expect("havuzdan atama yetkisi var");
    // E12/S4: kaybeden sahip yok → TEK satır, yalnız alma.
    assert_eq!(entries.len(), 1, "havuzdan atama TEK satır yazar");
    assert_eq!(entries[0].action, "claim_taken:self__creditAnalyst");
    let input = entries[0].input.as_ref().unwrap();
    assert_eq!(input["global_action"], json!("assign_from_pool"));
    assert_eq!(input["via"], json!("admin_assigned"));
    assert_eq!(input["owner"], json!(target.user_id.to_string()));

    // Sahipli iş → `reassign` yetkisi gerekir, listede YOK
    let claimed = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );
    let err = engine
        .reassign(&wfd, &claimed, &admin, Some(&target), None, Utc::now())
        .await
        .expect_err("kişiden kişiye devir ayrı yetkidir");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Escalation müdahalesi de listeye girdi: `wf_admin` olmak artık yetmiyor.
#[tokio::test]
async fn escalation_intervention_requires_its_own_power() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::Cancel]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let err = engine
        .skip_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect_err("skip_escalation yetkisi yok");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
    let err = engine
        .admin_fire_escalation(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect_err("fire_escalation yetkisi yok");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

/// Çoklu kural: yetki kümesi BİRLEŞİMDİR (ilk eşleşen kural kazanmaz).
#[tokio::test]
async fn multiple_rules_union_their_powers() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let mut wfd = golden_with_admin_actions(&[GlobalAction::Cancel]);
    let mut second = wfd.wf_admin[0].clone();
    second.allowed_global_actions = vec![GlobalAction::SendBack];
    wfd.wf_admin.push(second);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let powers = engine
        .admin_global_actions(&wfd, &wfes, &admin)
        .await
        .unwrap();
    assert!(
        powers.contains(&GlobalAction::Cancel) && powers.contains(&GlobalAction::SendBack),
        "iki kuralın kümesi birleşmeli: {powers:?}"
    );
}

/// `send_back` uğranmış bir node'a taşır ve WFAH'a GERÇEK admin ile yazılır.
#[tokio::test]
async fn admin_send_back_moves_to_visited_node_with_real_actor() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::SendBack]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at_visited(
        "parent__creditDeptManager",
        Some(manager(orgu).user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    let commit = engine
        .admin_send_back(&wfd, &wfes, &admin, "self__creditAnalyst", Utc::now())
        .await
        .expect("uğranmış node'a geri gönderilebilmeli");
    assert_eq!(
        commit.outcome,
        CommitOutcome::MoveTo {
            node: "self__creditAnalyst".into()
        }
    );
    let entry = &commit.wfah_entries[0];
    assert_eq!(entry.action, "admin:send_back");
    assert_eq!(
        entry.actor.user_id, admin.user_id,
        "iz 'system' değil GERÇEK admin olmalı"
    );
    assert_eq!(
        commit.new_dynctx,
        *wfes.dynctx.as_value(),
        "global aksiyon $ctx'e yazmaz"
    );
}

/// Uğranmamış node'a "geri" göndermek ileri atlamadır — K-2 süzgeci adminde de işler.
#[tokio::test]
async fn admin_send_back_rejects_unvisited_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::SendBack]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let err = engine
        .admin_send_back(&wfd, &wfes, &admin, "self__branchManager", Utc::now())
        .await
        .expect_err("uğranmamış hedef reddedilmeli");
    assert!(matches!(err, EngineError::TargetInvalid(_)), "{err:?}");
}

/// Bulunulan node'a geri gönderme işlemsizdir → reddedilir.
#[tokio::test]
async fn admin_send_back_rejects_current_node() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::SendBack]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let err = engine
        .admin_send_back(&wfd, &wfes, &admin, "self__creditAnalyst", Utc::now())
        .await
        .expect_err("bulunulan node reddedilmeli");
    assert!(matches!(err, EngineError::TargetInvalid(_)), "{err:?}");
}

/// `send_to_start` tek start kuralında hedef İSTEMEZ ve start node'una döner —
/// YENİ WFE açılmaz, aynı örnek geri sarar (`wfe_id` DEĞİŞMEZ).
#[tokio::test]
async fn admin_send_to_start_rewinds_same_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::SendToStart]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let commit = engine
        .admin_send_to_start(&wfd, &wfes, &admin, None, Utc::now())
        .await
        .expect("başa gönderilebilmeli");
    assert_eq!(
        commit.outcome,
        CommitOutcome::MoveTo {
            node: "type_branch__branchClerk".into()
        },
        "start[].from'a dönmeli"
    );
    assert_eq!(commit.wfe_id, wfes.wfe_id, "yeni WFE açılmamalı");
    assert_eq!(commit.wfah_entries[0].action, "admin:send_to_start");
}

/// `send_to_start` hedefi START node'u olmak zorunda: aksi halde `send_back` kapısını
/// atlamanın yolu olurdu.
#[tokio::test]
async fn admin_send_to_start_rejects_non_start_target() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::SendToStart]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at_visited(
        "parent__creditDeptManager",
        Some(manager(orgu).user_id),
        start_input(),
        vec!["self__creditAnalyst".into()],
    );

    let err = engine
        .admin_send_to_start(&wfd, &wfes, &admin, Some("self__creditAnalyst"), Utc::now())
        .await
        .expect_err("start olmayan hedef reddedilmeli");
    assert!(matches!(err, EngineError::TargetInvalid(_)), "{err:?}");
}

/// `cancel` WFE'yi terminal-class `terminated`a sokar; sebep makine-okunur.
#[tokio::test]
async fn admin_cancel_terminates_with_reason() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_admin_actions(&[GlobalAction::Cancel]);

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );

    let commit = engine
        .admin_cancel(&wfd, &wfes, &admin, Some("müşteri vazgeçti"), Utc::now())
        .await
        .expect("iptal edilebilmeli");
    match &commit.outcome {
        CommitOutcome::Terminated { end_response } => {
            assert_eq!(end_response["reason"], json!("ADMIN.Cancelled"));
            assert_eq!(end_response["note"], json!("müşteri vazgeçti"));
        }
        other => panic!("Terminated beklendi: {other:?}"),
    }
    assert_eq!(commit.wfah_entries[0].action, "admin:cancel");
    assert_eq!(commit.wfah_entries[0].actor.user_id, admin.user_id);
    assert!(
        commit.end_terminal.is_none(),
        "iptal başarılı bir terminal DEĞİL: end_terminal NULL kalmalı"
    );
    assert!(
        commit.staged_calls.is_empty(),
        "iptal ardıl akış TETİKLEMEZ"
    );
}

/// Terminal WFE üzerinde global aksiyon REDDEDİLİR — `cancel` dâhil (idempotent
/// "zaten iptal" bir cevap değil, çakışmadır: ikinci iptal WFAH'a ikinci kayıt yazardı).
#[tokio::test]
async fn global_actions_rejected_on_terminal_wfe() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None); // tam yetkili

    let orgu = Uuid::new_v4();
    let admin = manager(orgu);
    let mut wfes = wfes_at(
        "self__creditAnalyst",
        Some(analyst(orgu).user_id),
        start_input(),
    );
    wfes.status = WfeStatus::Terminated;

    for err in [
        engine
            .admin_cancel(&wfd, &wfes, &admin, None, Utc::now())
            .await
            .expect_err("cancel reddedilmeli"),
        engine
            .admin_send_back(&wfd, &wfes, &admin, "type_branch__branchClerk", Utc::now())
            .await
            .expect_err("send_back reddedilmeli"),
        engine
            .admin_send_to_start(&wfd, &wfes, &admin, None, Utc::now())
            .await
            .expect_err("send_to_start reddedilmeli"),
    ] {
        assert!(matches!(err, EngineError::WfeTerminal), "{err:?}");
    }
}

/// `wf_admin` kuralına UYMAYAN aktör hiçbir global aksiyon alamaz — liste dolu olsa da.
#[tokio::test]
async fn non_admin_gets_no_global_actions() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = test_engine(&org, &runner);
    let wfd = golden_with_wf_admin(None); // yetkili = branchManager

    let orgu = Uuid::new_v4();
    let outsider = analyst(orgu); // admin DEĞİL
    let wfes = wfes_at("self__creditAnalyst", Some(outsider.user_id), start_input());

    assert!(engine
        .admin_global_actions(&wfd, &wfes, &outsider)
        .await
        .unwrap()
        .is_empty());
    let err = engine
        .admin_cancel(&wfd, &wfes, &outsider, None, Utc::now())
        .await
        .expect_err("admin olmayan iptal edemez");
    assert!(matches!(err, EngineError::Unauthorized), "{err:?}");
}

// ============================ Ç2/Ç3/Ç4 — satır alanları (v2.3, WOR-75) ============
//
// Ç2: akış izi (`from_node`/`to_node`) ve Ç4: kol etiketi (`branch_entry`) artık
// SATIRIN kendisinde durur. Eskiden adapter bir commit'in TÜM satırlarına aynı
// from/to'yu yazıyordu; `$valid` satır satır hesaplandığı için bu yanlış cevap
// üretir. Ç3: kol marker'ları kolun KİMLİĞİNİ (`branch_entry`) ve KONUMUNU
// (`at_node`) ayrı alanlarda taşır.

/// Kol içinde ilerlemiş (`BranchMoveTo` görmüş) kol: kimliği giriş node'u KALIR,
/// konumu değişir.
fn moved_branch(
    entry: &str,
    at: &str,
    status: BranchStatus,
    claimed_by: Option<Uuid>,
) -> BranchState {
    let mut b = branch(at, status, claimed_by);
    b.entry_node = entry.into();
    b
}

/// Ç2: hareket üreten satır commit'in from/to'sunu taşır, aynı commit'teki marker
/// satırları (trigger kaydı) TAŞIMAZ — ve ayrım marker ADINDAN türetilmez.
#[tokio::test(start_paused = true)]
async fn movement_row_carries_the_path_but_markers_do_not() {
    let org = MockOrg {
        role_assigned: true,
    };
    // within_limit false → hedef bir NODE (terminal değil), yani to_node dolu.
    let runner = MockRunner::ok(650, "C", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
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
            None,
        )
        .await
        .unwrap();

    let (movement, markers) = commit.wfah_entries.split_first().unwrap();
    assert_eq!(movement.action, "analyst_approve");
    assert_eq!(movement.from_node.as_deref(), Some("self__creditAnalyst"));
    assert_eq!(movement.to_node.as_deref(), Some("self__branchManager"));
    // Paralel mod DIŞI → satır bir kolda değil.
    assert!(movement.branch_entry.is_none());
    assert!(
        !markers.is_empty(),
        "golden'ın analyst_approve'u trigger taşır: {:?}",
        wfah_actions(&commit)
    );
    for m in markers {
        assert!(
            m.from_node.is_none() && m.to_node.is_none(),
            "marker satırı akış izi taşımamalı: {}",
            m.action
        );
    }
}

/// Start satırının `from_node`'u YOKTUR (öncesi yok), `to_node`'u varılan node'dur.
#[tokio::test(start_paused = true)]
async fn start_row_carries_only_the_target() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(750, "A", true);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };

    let new = engine
        .start(
            &golden(),
            &clerk(Uuid::new_v4()),
            Uuid::nil(),
            None,
            &start_input(),
            Uuid::new_v4(),
            None,
        )
        .await
        .unwrap();

    let row = &new.wfah_entries[0];
    assert_eq!(row.action, "create_application");
    assert!(row.from_node.is_none(), "start'ın öncesi yok");
    assert_eq!(row.to_node.as_deref(), Some("self__creditAnalyst"));
    assert!(row.branch_entry.is_none());
}

/// Ç3: kol içinde ilerlemiş bir kol iptal olduğunda marker KİMLİĞİ ve KONUMU AYRI
/// alanlarda taşır. Tek `node` alanı KONUMU yazıyordu — `$valid` eleme kuralı 1 ve
/// portalın kol eşleştirmesi kimlik bekler, o yüzden yanlış anahtarı okuyorlardı.
#[tokio::test]
async fn branch_markers_separate_identity_from_position() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let wfes = parallel_wfes(
        vec![
            // finance kolu giriş node'undan ilerledi: kimlik financeApprover KALIR.
            moved_branch(
                "self__financeApprover",
                "self__financeSenior",
                BranchStatus::Active,
                None,
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );

    // SLA-3 sonlanması: acting kol YOK, tüm aktif kollar iptal edilir.
    let commit = engine.fire_deadline_timeout(&wfes, Utc::now());

    let moved = commit
        .wfah_entries
        .iter()
        .find(|e| {
            e.action == "_branch_cancelled"
                && e.branch_entry.as_deref() == Some("self__financeApprover")
        })
        .expect("ilerlemiş kol için _branch_cancelled");
    let input = moved.input.as_ref().unwrap();
    assert_eq!(input["branch_entry"], json!("self__financeApprover"));
    assert_eq!(input["at_node"], json!("self__financeSenior"));
    assert!(
        input.get("node").is_none(),
        "belirsiz `node` alanı KALKTI: {input}"
    );

    // Manşetin listeleri de KİMLİK taşır (konum değil).
    let summary = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "_collapse")
        .and_then(|e| e.input.as_ref())
        .expect("_collapse özeti");
    assert_eq!(
        summary["cancelled"],
        json!(["self__financeApprover", "self__legalApprover"])
    );

    // Ç2: bu commit'te hareket üreten satır YOK (WFE terminated) — hepsi marker.
    for e in &commit.wfah_entries {
        assert!(
            e.from_node.is_none() && e.to_node.is_none(),
            "{} akış izi taşımamalı",
            e.action
        );
    }
}

/// Ç3 (kanıt #1): `_branch_superseded`in onay bilgisi kol HAREKET ETTİKTEN sonra da
/// bulunur — geri okuma anahtarı kolun kimliğidir. `branch_node` ile aranırken çok
/// adımlı kolda `approved_by: null` yazılıyordu.
#[tokio::test]
async fn superseded_marker_finds_the_approval_after_the_branch_moved() {
    let org = MockOrg {
        role_assigned: true,
    };
    let runner = MockRunner::ok(0, "-", false);
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let senior = actor_with_role("financeSenior");
    let mut wfes = parallel_wfes(
        vec![
            // Kol ara node'dan join'e vardı; kimliği hâlâ giriş node'u.
            moved_branch(
                "self__financeApprover",
                "self__financeSenior",
                BranchStatus::Arrived,
                None,
            ),
            branch("self__legalApprover", BranchStatus::Active, None),
        ],
        join_node(),
        parallel_ctx(),
    );
    let approved_at = Utc::now();
    // Varış marker'ı Ç3 şeklinde: kimlik `branch_entry`, konum `at_node`.
    let seq = wfes.wfah.entries().last().unwrap().seq + 1;
    wfes.wfah = Wfah(
        wfes.wfah
            .entries()
            .iter()
            .cloned()
            .chain(std::iter::once(WfahEntry {
                seq,
                action: "_branch_arrived".into(),
                actor: senior.clone(),
                input: Some(json!({
                    "branch_entry": "self__financeApprover",
                    "at_node": "self__financeSenior",
                    "approved_by": senior,
                    "approved_at": approved_at,
                    "claimed_at": approved_at,
                })),
                applied_at: approved_at,
                from_node: None,
                to_node: None,
                branch_entry: Some("self__financeApprover".into()),
                branch_round: Some(1),
            }))
            .collect(),
    );

    let commit = engine.fire_deadline_timeout(&wfes, Utc::now());

    let superseded = commit
        .wfah_entries
        .iter()
        .find(|e| e.action == "_branch_superseded")
        .and_then(|e| e.input.as_ref())
        .expect("arrived kol için _branch_superseded");
    assert_eq!(superseded["branch_entry"], json!("self__financeApprover"));
    assert_eq!(superseded["at_node"], json!("self__financeSenior"));
    assert_eq!(
        superseded["approved_by"]["user_id"],
        json!(senior.user_id),
        "onay bilgisi kol hareketinden sonra da bulunmalı: {superseded}"
    );
}
