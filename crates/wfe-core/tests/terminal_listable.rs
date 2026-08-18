//! Terminal-seviyesi görünürlük — `terminals[].listable[]` (2026-08-17).
//!
//! Cevaplanan ürün sorusu: "bu akış bittikten sonra kimler görebilir?" Kök
//! `listable[]` bunu SONUÇTAN BAĞIMSIZ cevaplıyordu; terminal'ler zaten sonucu
//! ayırdığı için grant oraya konunca ayrım YAPIDAN geliyor — "onaylandı"yı gören
//! ile "reddedildi"yi gören `when` guard'ı yazmadan ayrılabiliyor.
//!
//! Dosya ÜÇ kapıyı birden bekliyor, çünkü üçü ayrışırsa hata sessiz olur:
//!   1. **şema** — `terminals[]` `additionalProperties: false` taşır; alan şemaya
//!      eklenmemiş olsaydı geçerli belge de reddedilirdi.
//!   2. **`can_view` (g)** — referans okuma. `status != Active` erken dönüşünün
//!      ÜSTÜNDE olmak zorunda, yoksa kriter hiç çalışmaz.
//!   3. **projeksiyon** (`Engine::terminal_view_grants`) — `wf.wfe.end_view_c_a`
//!      kolonuna yazılan çözülmüş liste. (2) ile (3) ayrışırsa liste ucu ile detay
//!      ucu farklı cevap verir; `visibility_report` tam bunu ölçüyor.

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;
use wfe_core::error::EngineError;
use wfe_core::ports::OrgPort;
use wfe_core::types::actor::{Actor, OrgUnit};
use wfe_core::types::dynctx::DynCtx;
use wfe_core::types::wfah::Wfah;
use wfe_core::types::wfd_v22::{AutoexecDef, JoinRule, Wfd};
use wfe_core::types::wfe::WfeStatus;
use wfe_core::v22::pipeline::Engine;
use wfe_core::v22::ports::{AutoexecRunner, ExecEnv, ExecFailure, Wfes};
use wfe_core::v22::visibility::can_view;

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");

// ── şema kapısı (docs/spec/schema.json) ─────────────────────────────────────

#[test]
fn schema_gate_accepts_terminal_listable() {
    let mut doc = base_doc();
    doc["terminals"][0]["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["auditor"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.credit_info.amount_requested > 1" }
    ]);
    Wfd::from_value_checked(doc).expect("geçerli terminal listable şema kapısından geçmeli");
}

#[test]
fn schema_gate_rejects_unknown_field_in_terminal_rule() {
    let mut doc = base_doc();
    doc["terminals"][0]["listable"] =
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] }, "sarkan": true }]);
    assert!(
        Wfd::from_value_checked(doc).is_err(),
        "kural içinde bilinmeyen alan reddedilmeli (listableRule additionalProperties: false)"
    );
}

#[test]
fn schema_gate_rejects_empty_c_r_in_terminal_listable() {
    // `"c_r": []` serde için geçerli görünür (boş Vec) — kapı ŞEMADIR (minItems).
    let mut doc = base_doc();
    doc["terminals"][0]["listable"] = json!([{ "c_a": { "c_orgu": "self", "c_r": [] } }]);
    assert!(
        Wfd::from_value_checked(doc).is_err(),
        "boş c_r reddedilmeli (candidateActor $ref'i terminal'de de işlemeli)"
    );
}

/// Alan verilmezse boştur ve yeniden serileştirmede HİÇ çıkmaz — terminal
/// listable taşımayan belgeler birebir aynı serileşir (golden fixture korunur).
#[test]
fn missing_terminal_listable_is_empty_and_not_serialized() {
    let wfd = golden();
    assert!(wfd.terminals.iter().all(|t| t.listable.is_empty()));
    let round = serde_json::to_value(&wfd).expect("serialize");
    for t in round["terminals"].as_array().expect("terminals dizisi") {
        assert!(
            t.get("listable").is_none(),
            "boş terminal listable serileştirmede görünmemeli: {t}"
        );
    }
}

/// Şekil kök `listable` ile AYNI (`CaGrantRule`) — aynı JSON iki yerde de okunur.
#[test]
fn terminal_listable_shares_the_rule_shape_with_root_listable() {
    let rule = json!({ "c_a": { "c_orgu": "self", "c_r": ["auditor"] }, "when": "true" });
    let mut doc = base_doc();
    doc["listable"] = json!([rule.clone()]);
    doc["terminals"][0]["listable"] = json!([rule]);
    let wfd = Wfd::from_value(doc).expect("parse");
    assert_eq!(wfd.listable.len(), 1);
    assert_eq!(wfd.terminals[0].listable.len(), 1);
    assert_eq!(wfd.terminals[0].listable[0].when, wfd.listable[0].when);
}

// ── can_view (g) ────────────────────────────────────────────────────────────

/// Asıl kazanım: aynı belgede İKİ terminal, İKİ farklı görünürlük. Denetçi yalnız
/// REDDEDİLEN akışları görür; onaylananları görmez. Kök `listable` ile bu ayrım
/// yapılamıyordu (kural sonucu bilmiyor).
#[tokio::test]
async fn terminal_listable_is_scoped_to_the_terminal_that_was_reached() {
    let org = MockOrg;
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] } }]),
    );
    let auditor = role_actor("auditor");

    let rejected = finished_at(Some("terminal_rejected"));
    assert!(
        can_view(&wfd, &rejected, &auditor, &org).await.unwrap(),
        "reddedilen akışta terminal grant'ı eşleşmeli"
    );

    let approved = finished_at(Some("terminal_approved"));
    assert!(
        !can_view(&wfd, &approved, &auditor, &org).await.unwrap(),
        "grant BAŞKA terminal'e yazılmıştı — onaylanan akışa SIZMAMALI"
    );
}

/// Kriter (g) `status != Active` erken dönüşünün ÜSTÜNDEDİR. Bu test tam o satırın
/// bekçisi: kriter yanlışlıkla aşağı taşınırsa buradan başka hiçbir şey patlamaz
/// (grant zaten yalnız bitmiş satırda sorulur).
#[tokio::test]
async fn terminal_listable_survives_the_inactive_early_return() {
    let org = MockOrg;
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] } }]),
    );
    let auditor = role_actor("auditor");

    let mut wfes = finished_at(Some("terminal_rejected"));
    wfes.status = WfeStatus::Terminal;
    assert!(can_view(&wfd, &wfes, &auditor, &org).await.unwrap());
}

/// `Failed` (error) ve `Terminated` (SLA) yollarında VARILMIŞ bir terminal yoktur —
/// `end_terminal` `None` kalır ve kriter hiç çalışmaz. `end_terminal` kolonundan
/// ÖNCE bitmiş eski satırlar da bu yoldan geçer (eski davranış korunur).
#[tokio::test]
async fn no_terminal_means_no_terminal_grant() {
    let org = MockOrg;
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] } }]),
    );
    let auditor = role_actor("auditor");

    for status in [WfeStatus::Error, WfeStatus::Terminated] {
        let mut wfes = finished_at(None);
        wfes.status = status.clone();
        assert!(
            !can_view(&wfd, &wfes, &auditor, &org).await.unwrap(),
            "{status:?}: terminal'e varılmadı, terminal grant'ı da olmamalı"
        );
    }
}

/// Grant ACT/claim VERMEZ ve AKTİF WFE'yi göstermez: `end_terminal` dolmadan kriter
/// açılmaz. (Node `listable`ının aynadaki karşılığı — o da terminal'de kapanıyor.)
#[tokio::test]
async fn terminal_listable_does_not_leak_into_the_active_wfe() {
    let org = MockOrg;
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] } }]),
    );
    let auditor = role_actor("auditor");

    let mut active = finished_at(None);
    active.status = WfeStatus::Active;
    active.current_node = Some("self__creditAnalyst".into());
    assert!(!can_view(&wfd, &active, &auditor, &org).await.unwrap());
}

/// `when` guard'ı kök `listable` ile AYNI biçimde uygulanır (aynı matcher).
#[tokio::test]
async fn terminal_listable_when_guard_is_applied() {
    let org = MockOrg;
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{
            "c_a": { "c_orgu": "self", "c_r": ["auditor"] },
            "when": "$ctx.credit_info.amount_requested >= 100000"
        }]),
    );
    let auditor = role_actor("auditor");

    let big = finished_at_with_ctx(Some("terminal_rejected"), start_input(150_000));
    assert!(can_view(&wfd, &big, &auditor, &org).await.unwrap());

    let small = finished_at_with_ctx(Some("terminal_rejected"), start_input(30_000));
    assert!(!can_view(&wfd, &small, &auditor, &org).await.unwrap());
}

// ── projeksiyon (Engine::terminal_view_grants) ──────────────────────────────

/// Referans okuma (`can_view` (g)) ile kolon (`end_view_c_a`) AYNI kuralı ifade
/// eder. Burada ölçülen: `when` guard'ı projeksiyonda da uygulanıyor mu — aksi
/// halde kolon "false" bir kuralı da yazar ve liste ucu detay ucundan sapar.
#[tokio::test]
async fn projection_resolves_terminal_rules_and_applies_the_guard() {
    let org = MockOrg;
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{
            "c_a": { "c_orgu": "self", "c_r": ["auditor"] },
            "when": "$ctx.credit_info.amount_requested >= 100000"
        }]),
    );
    let origin = Uuid::new_v4();

    let hit = engine
        .terminal_view_grants(
            &wfd,
            "terminal_rejected",
            &start_input(150_000),
            &Wfah::empty(),
            Uuid::new_v4(),
            origin,
            Uuid::nil(),
        )
        .await
        .unwrap();
    assert_eq!(hit.len(), 1, "guard true → aday çözülmeli");

    let miss = engine
        .terminal_view_grants(
            &wfd,
            "terminal_rejected",
            &start_input(30_000),
            &Wfah::empty(),
            Uuid::new_v4(),
            origin,
            Uuid::nil(),
        )
        .await
        .unwrap();
    assert!(miss.is_empty(), "guard false → kolona hiçbir aday yazılmamalı");
}

/// Bilinmeyen terminal = boş liste, HATA DEĞİL. `node_view_grants` ile aynı
/// gerekçe: bu bir yetki sorusu değil cache üretimidir; eksik kayıtta hata atmak
/// commit'i düşürürdü.
#[tokio::test]
async fn projection_of_unknown_terminal_is_empty_not_an_error() {
    let org = MockOrg;
    let runner = NoRunner;
    let engine = Engine {
        org: &org,
        exec: &runner,
        env: Default::default(),
    };

    let out = engine
        .terminal_view_grants(
            &golden(),
            "boyle_bir_terminal_yok",
            &start_input(30_000),
            &Wfah::empty(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            Uuid::nil(),
        )
        .await
        .expect("bilinmeyen terminal hata ÜRETMEMELİ");
    assert!(out.is_empty());
}

// ── validator ───────────────────────────────────────────────────────────────

/// `$actor` grant guard'larında YASAK (`grant_when_actor_ref`): projeksiyon viewer
/// bilinmezken yazılır, viewer'a bağlı bir guard kolona sığmaz. Kural terminal
/// listable'da da geçerli olmalı — olmasaydı aynı yasağın kaçış deliği açılırdı.
#[test]
fn validator_rejects_actor_ref_in_terminal_listable_guard() {
    let wfd = wfd_with_terminal_listable(
        "terminal_rejected",
        json!([{ "c_a": { "c_orgu": "self", "c_r": ["auditor"] }, "when": "$actor.role == \"x\"" }]),
    );
    let report = wfe_core::validator::validate(&wfd);
    // `grant_when_actor_ref` bir HATA kodudur (yayını engeller) — uyarı değil.
    assert!(
        report
            .errors
            .iter()
            .any(|i| i.code == "grant_when_actor_ref"
                && i.path.starts_with("terminals[terminal_rejected].listable")),
        "terminal listable guard'ında $actor yasağı işlemeli: {:?}",
        report.errors
    );
}

// ── yardımcılar ─────────────────────────────────────────────────────────────

fn base_doc() -> Value {
    serde_json::from_str(FIXTURE).expect("fixture JSON")
}

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).unwrap()
}

fn wfd_with_terminal_listable(terminal_id: &str, listable: Value) -> Wfd {
    let mut doc = base_doc();
    let terminals = doc["terminals"].as_array_mut().expect("terminals dizisi");
    let t = terminals
        .iter_mut()
        .find(|t| t["id"] == terminal_id)
        .unwrap_or_else(|| panic!("fixture'da {terminal_id} yok"));
    t["listable"] = listable;
    Wfd::from_value(doc).expect("parse")
}

fn start_input(amount_requested: i64) -> Value {
    json!({
        "applicant": {"name": "Ayşe Yılmaz", "tckid": "12345678901", "income": 30000},
        "credit_info": {"amount_requested": amount_requested, "purpose": "ev tadilatı"}
    })
}

fn role_actor(role: &str) -> Actor {
    Actor {
        orgu_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: role.into(),
    }
}

/// Bitmiş WFE: `current_node` NULL, assignment temiz — terminal commit'inin
/// bıraktığı durumun aynısı. `end_terminal` testin değişkeni.
fn finished_at(end_terminal: Option<&str>) -> Wfes {
    finished_at_with_ctx(end_terminal, start_input(30_000))
}

fn finished_at_with_ctx(end_terminal: Option<&str>, ctx: Value) -> Wfes {
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
        status: WfeStatus::Terminal,
        current_node: None,
        end_terminal: end_terminal.map(str::to_string),
        assigned_to: None,
        end_response: None,
        deadline: None,
        claimed_at: None,
        created_at,
        branches: vec![],
        join_target: None,
        join_rule: JoinRule::All,
        origin_orgu_id: None,
    }
}

/// `visibility_view.rs`teki mock'un aynısı: her ifade aktörün çapasına çözülür,
/// rol daima atanmış sayılır → test ROL kanalını ve `when` guard'ını ölçer, org
/// ağacı çözümünü değil.
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

struct NoRunner;

#[async_trait]
impl AutoexecRunner for NoRunner {
    async fn run(&self, _def: &AutoexecDef, _env: &ExecEnv) -> Result<Value, ExecFailure> {
        unreachable!("bu testlerde autoexec koşmaz")
    }
}
