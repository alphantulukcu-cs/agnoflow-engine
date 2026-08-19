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
        expect_start_reject: false,
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
        active: None,
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
            target: None,
            expect_reject: false,
        },
        ScenarioStep::Action {
            action: "ikinci".into(),
            actor: Some(sc_actor("creditAnalyst")),
            input: json!({}),
            node: None,
            target: None,
            expect_reject: false,
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

// ── paralel kol ve WFC çağrı dönüşü ─────────────────────────────────────────

const PARALLEL: &str = include_str!("../../wfe-core/tests/fixtures/paralel-onay.json");
const CALLER: &str = include_str!("../../wfe-core/tests/fixtures/akis-cagrisi.json");

fn parallel_scenario() -> Scenario {
    let mut s = base_scenario();
    s.start_actor = Some(sc_actor("requester"));
    s.start_input = json!({"request": {"title": "Sunucu alımı", "amount": 150000}});
    s
}

/// Paralel kolda adım, `node` ile hangi kola uygulandığını söyleyebilmeli.
#[tokio::test]
async fn parallel_branch_step_targets_its_branch_via_node() {
    let wfd = Wfd::from_json(PARALLEL).unwrap();
    let json_doc: serde_json::Value = serde_json::from_str(PARALLEL).unwrap();
    let mut s = parallel_scenario();
    s.steps = vec![
        ScenarioStep::Action {
            action: "start_review".into(),
            actor: Some(sc_actor("coordinator")),
            input: json!({}),
            node: None,
            target: None,
            expect_reject: false,
        },
        ScenarioStep::Action {
            action: "approve".into(),
            actor: Some(sc_actor("financeApprover")),
            input: json!({}),
            node: Some("self__financeApprover".into()),
            target: None,
            expect_reject: false,
        },
    ];
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 2);
}

/// `node` verilmezse paralel modda aynı adım belirsizdir ve senaryo kalır —
/// koşucunun kol seçimini gerçekten ilettiğinin kanıtı.
#[tokio::test]
async fn parallel_branch_step_without_node_fails() {
    let wfd = Wfd::from_json(PARALLEL).unwrap();
    let json_doc: serde_json::Value = serde_json::from_str(PARALLEL).unwrap();
    let mut s = parallel_scenario();
    s.steps = vec![
        ScenarioStep::Action {
            action: "start_review".into(),
            actor: Some(sc_actor("coordinator")),
            input: json!({}),
            node: None,
            target: None,
            expect_reject: false,
        },
        ScenarioStep::Action {
            action: "approve".into(),
            actor: Some(sc_actor("financeApprover")),
            input: json!({}),
            node: None,
            target: None,
            expect_reject: false,
        },
    ];
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(!res.ok, "node'suz kol adımı geçmemeliydi");
    assert_eq!(res.steps_executed, 1, "ilk adım geçti, ikincisi kaldı");
}

/// WFC durağı: `self__creditAnalyst` bir `mode: wait` çağrı node'udur; akış
/// çağrı dönüşü verilene kadar ilerlemez. `call_return` adımı onu çözer ve
/// skor >= 700 dalı `self__branchManager`'a götürür, oradan terminale.
#[tokio::test]
async fn call_return_step_resumes_a_waiting_call_through_to_terminal() {
    let wfd = Wfd::from_json(CALLER).unwrap();
    let json_doc: serde_json::Value = serde_json::from_str(CALLER).unwrap();
    let mut s = base_scenario();
    s.start_input = json!({"basvuru": {"musteri_no": "M1", "tutar": 50000}});
    s.steps = vec![
        ScenarioStep::CallReturn {
            call_return: wf_wfe::scenario::CallReturn {
                status: "completed".into(),
                result: Some(json!({ "skor": 750, "karar": "olumlu" })),
            },
        },
        ScenarioStep::Action {
            action: "manager_decide".into(),
            actor: Some(sc_actor("branchManager")),
            input: json!({ "kullandirim_tutari": 50000 }),
            node: None,
            target: None,
            expect_reject: false,
        },
    ];
    s.expect = Some(Expect {
        terminal: None,
        context_contains: Some(json!({ "skor": 750 })),
        active: None,
    });
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 2);
    assert!(res.terminal, "manager_decide sonrası terminale ulaşılmalı");
}

/// Başarısız çağrı dönüşü akışı `terminal_rejected`'a götürür — `status`
/// alanının koşucudan motora gerçekten geçtiğinin kanıtı.
#[tokio::test]
async fn failed_call_return_status_reaches_the_engine() {
    let wfd = Wfd::from_json(CALLER).unwrap();
    let json_doc: serde_json::Value = serde_json::from_str(CALLER).unwrap();
    let mut s = base_scenario();
    s.start_input = json!({"basvuru": {"musteri_no": "M1", "tutar": 50000}});
    s.steps = vec![ScenarioStep::CallReturn {
        call_return: wf_wfe::scenario::CallReturn {
            status: "failed".into(),
            result: None,
        },
    }];
    let res = run(&engine(), &wfd, &json_doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.terminal, "başarısız çağrı akışı terminale götürür");
    assert_eq!(res.steps_executed, 1);
}

// ── belge kapısı + not adımları (2026-08-19) ─────────────────────────────────
//
// Portal kullanıcısının gerçekte yaptığı iki şey senaryoda da yapılabiliyor:
// belge yükle (`attach`) ve not yaz (`note`). Kapı kuralı gerçek akışla TEK
// kaynaktan gelir (`wfe_core::v22::attachments`), "yüklenmiş" tanımı sim'de
// `SimState.attachments`tır. `expectReject` ise NEGATİF testi mümkün kılar:
// "belge yüklenmeden onaylanamaz" artık kanıtlanabilir bir senaryodur.

const BELGE: &str = include_str!("../../wfe-core/tests/fixtures/belge-onay.json");

fn belge() -> (Wfd, serde_json::Value) {
    (
        Wfd::from_json(BELGE).unwrap(),
        serde_json::from_str(BELGE).unwrap(),
    )
}

/// belge-onay fixture'ının start kuralı: `create_application` (branchClerk).
fn belge_scenario() -> Scenario {
    let mut s = base_scenario();
    s.start_input = json!({
        "applicant": { "name": "Ayşe", "tckid": "1" },
        "credit_info": { "amount_requested": 100000 }
    });
    s
}

fn attach_step(group: &str, item: &str, ct: Option<&str>, size: i64) -> ScenarioStep {
    ScenarioStep::Attach {
        attach: wf_wfe::scenario::AttachStep {
            group: group.into(),
            item: item.into(),
            filename: Some("dosya.pdf".into()),
            content_type: ct.map(str::to_string),
            size_bytes: size,
            expect_reject: false,
        },
    }
}

fn analyst_approve(expect_reject: bool) -> ScenarioStep {
    ScenarioStep::Action {
        action: "analyst_approve".into(),
        actor: Some(sc_actor("creditAnalyst")),
        input: json!({ "credit_info": { "amount_requested": 100000 } }),
        node: None,
        target: None,
        expect_reject,
    }
}

/// Zorunlu belgeler yüklenmeden aksiyon KAPIDA durur — ve `expectReject: true`
/// olduğu için senaryo GEÇER (kanıtladığı şey: kapı devrede).
#[tokio::test]
async fn document_gate_blocks_the_action_and_expect_reject_passes() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![analyst_approve(true)];
    s.expect = Some(Expect {
        terminal: None,
        context_contains: None,
        active: Some(true), // kapı tuttuysa akış hâlâ analist havuzunda
    });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 1);
    assert_eq!(res.rejected_as_expected.len(), 1);
    assert!(
        res.rejected_as_expected[0].contains("basvuru_belgeleri/kimlik"),
        "eksik belge ADIYLA yazılmalı: {:?}",
        res.rejected_as_expected
    );
}

/// Aynı kapı `expectReject` olmadan senaryoyu KALDIRIR ve eksik belgeleri sayar.
#[tokio::test]
async fn document_gate_failure_names_every_missing_slot() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![analyst_approve(false)];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    let msg = &res.failures[0];
    assert!(msg.contains("basvuru_belgeleri/kimlik"), "{msg}");
    assert!(msg.contains("basvuru_belgeleri/gelir_belgesi"), "{msg}");
}

/// `attach` adımları kapıyı açar: iki zorunlu belge yüklenince aksiyon geçer.
#[tokio::test]
async fn attach_steps_open_the_gate() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 1024),
        attach_step(
            "basvuru_belgeleri",
            "gelir_belgesi",
            Some("application/pdf"),
            2048,
        ),
        analyst_approve(false),
    ];
    s.expect = Some(Expect {
        terminal: None,
        context_contains: Some(json!({ "credit_info": { "amount_requested": 100000 } })),
        active: Some(true), // müdür havuzuna geçti, hâlâ aktif
    });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 3);
    assert_eq!(res.attachments.len(), 2, "{:?}", res.attachments);
}

/// `required: false` slot kapı DEĞİLDİR; kapsamı `manager_decide` olan grup ise
/// yalnız o aksiyonu kapar — senaryo bu ayrımı uçtan uca kanıtlar.
#[tokio::test]
async fn scoped_group_only_gates_its_own_action() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        analyst_approve(false),
        // Müdür havuzundayız: `onay_belgeleri` YALNIZ `manager_decide`ı kapar.
        ScenarioStep::Action {
            action: "manager_decide".into(),
            actor: Some(sc_actor("branchManager")),
            input: json!({ "manager_decision": "approve" }),
            node: None,
            target: None,
            expect_reject: true, // kredi_raporu yüklenmedi
        },
        attach_step("onay_belgeleri", "kredi_raporu", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "manager_decide".into(),
            actor: Some(sc_actor("branchManager")),
            input: json!({ "manager_decision": "approve" }),
            node: None,
            target: None,
            expect_reject: false,
        },
    ];
    // NOT: `expect.terminal` KULLANILMADI — belge-onay fixture'ının terminalleri
    // `wfes_effects.set` taşımıyor, `infer_terminal_id` bu yüzden id çözemez
    // (bilinçli: belirsizlik sessiz yanlış pozitife dönüşmesin). Akışın BİTTİĞİ
    // `active: false` ile kanıtlanır.
    s.expect = Some(Expect {
        terminal: None,
        context_contains: Some(json!({ "manager_decision": "approve" })),
        active: Some(false),
    });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.terminal);
    assert_eq!(res.rejected_as_expected.len(), 1);
}

/// Reddedilmesi beklenen adım GEÇERSE senaryo kalır — kural delik demektir.
#[tokio::test]
async fn expect_reject_on_a_passing_step_fails_the_scenario() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        analyst_approve(true), // belgeler tam → geçecek, oysa ret bekleniyor
    ];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert!(
        res.failures[0].contains("reddedilmeliydi"),
        "{:?}",
        res.failures
    );
}

/// Katalogdaki format/boyut kuralı senaryoda da uygulanır: yanlış tip reddedilir.
#[tokio::test]
async fn attachment_format_rule_is_enforced_in_scenarios() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Attach {
        attach: wf_wfe::scenario::AttachStep {
            group: "basvuru_belgeleri".into(),
            item: "gelir_belgesi".into(),
            filename: Some("gelir.png".into()),
            content_type: Some("image/png".into()), // yalnız application/pdf kabul
            size_bytes: 10,
            expect_reject: true,
        },
    }];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.attachments.is_empty(), "reddedilen dosya yüklenmiş sayılmaz");
}

/// Boyut sınırı (5 MB) aşılırsa yükleme reddedilir.
#[tokio::test]
async fn attachment_size_limit_is_enforced_in_scenarios() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![attach_step(
        "basvuru_belgeleri",
        "kimlik",
        Some("application/pdf"),
        6 * 1024 * 1024,
    )];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("5"), "{:?}", res.failures);
}

/// Aktif adımda TOPLANMAYAN slota yükleme reddedilir (gerçek akışta `unknown_slot`).
#[tokio::test]
async fn attaching_to_a_slot_not_collected_here_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    // Start sonrası analist havuzundayız; `onay_belgeleri` yalnız müdür havuzunda.
    s.steps = vec![attach_step(
        "onay_belgeleri",
        "kredi_raporu",
        Some("application/pdf"),
        10,
    )];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert!(
        res.failures[0].contains("toplanmıyor"),
        "{:?}",
        res.failures
    );
}

/// Not adımı akışı ETKİLEMEZ (K1) ama kayda geçer ve limitleri denetlenir.
#[tokio::test]
async fn note_step_is_recorded_without_touching_the_flow() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        ScenarioStep::Note {
            note: wf_wfe::scenario::NoteStep {
                body: "Müşteri şubeyi aradı, ek belge yollayacak.".into(),
                audience: Default::default(),
                files: vec![wf_wfe::note_rules::NoteFileSpec {
                    filename: "telefon-notu.pdf".into(),
                    content_type: Some("application/pdf".into()),
                    size_bytes: 1024,
                }],
                actor: Some(sc_actor("creditAnalyst")),
                expect_reject: false,
            },
        },
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        analyst_approve(false),
    ];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.notes, 1);
    assert_eq!(res.steps_executed, 4);
    // Not `$ctx`'e YAZILMAZ — dynctx'te iz yok.
    assert!(
        !serde_json::to_string(&res.dynctx).unwrap().contains("şubeyi"),
        "not context'e sızdı: {}",
        res.dynctx
    );
}

/// Not limitleri (yasak MIME) senaryoda da uygulanır — gerçek akışla TEK kaynak.
#[tokio::test]
async fn note_file_blocklist_is_enforced_in_scenarios() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Note {
        note: wf_wfe::scenario::NoteStep {
            body: "kurulum dosyası".into(),
            audience: Default::default(),
            files: vec![wf_wfe::note_rules::NoteFileSpec {
                filename: "kur.sh".into(),
                content_type: Some("application/x-sh".into()),
                size_bytes: 10,
            }],
            actor: None,
            expect_reject: false,
        },
    }];
    let res = run(&engine(), &wfd, &doc, &s, Some(&fallback())).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("yasak"), "{:?}", res.failures);
}

/// Boş gövdeli not reddedilir; `expectReject` ile bu da bir testtir.
#[tokio::test]
async fn empty_note_body_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Note {
        note: wf_wfe::scenario::NoteStep {
            body: "   ".into(),
            audience: Default::default(),
            files: vec![],
            actor: None,
            expect_reject: true,
        },
    }];
    let res = run(&engine(), &wfd, &doc, &s, Some(&fallback())).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.notes, 0);
}

/// `active` beklentisi: akış bitmişse "hâlâ aktif" beklentisi KALIR.
#[tokio::test]
async fn active_expectation_catches_an_unexpected_finish() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        analyst_approve(false),
        attach_step("onay_belgeleri", "kredi_raporu", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "manager_decide".into(),
            actor: Some(sc_actor("branchManager")),
            input: json!({ "manager_decision": "reject" }),
            node: None,
            target: None,
            expect_reject: false,
        },
    ];
    s.expect = Some(Expect {
        terminal: None,
        context_contains: None,
        active: Some(true),
    });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("aktif"), "{:?}", res.failures);
}

/// Adım şekilleri JSON'dan DÖNÜŞTÜRMESİZ parse olur — editör bu şekli yazıyor.
#[test]
fn attach_and_note_steps_parse_from_editor_json() {
    let steps: Vec<ScenarioStep> = serde_json::from_value(json!([
        { "attach": { "group": "basvuru_belgeleri", "item": "kimlik",
                      "filename": "kimlik.pdf", "content_type": "application/pdf",
                      "size_bytes": 1024 } },
        { "note": { "body": "not", "files": [{ "filename": "a.pdf", "size_bytes": 5 }] } },
        { "action": "analyst_approve", "input": {}, "expectReject": true }
    ]))
    .unwrap();
    assert!(matches!(steps[0], ScenarioStep::Attach { .. }));
    assert!(matches!(steps[1], ScenarioStep::Note { .. }));
    assert!(
        matches!(&steps[2], ScenarioStep::Action { expect_reject: true, .. }),
        "expectReject camelCase okunmalı"
    );
}

/// `expectReject` bir KURAL reddi için vardır; senaryonun kendi eksiği (aktör
/// çözülememesi) onunla yutulamaz — yoksa aktörü unutulmuş senaryo "kural devrede"
/// diye geçerdi.
#[tokio::test]
async fn expect_reject_does_not_swallow_an_unresolved_actor() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Action {
        action: "analyst_approve".into(),
        actor: None, // ve fallback da verilmiyor
        input: json!({}),
        node: None,
        target: None,
        expect_reject: true,
    }];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok, "aktörsüz adım geçmemeli");
    assert!(res.failures[0].contains("aktör"), "{:?}", res.failures);
}

/// Senaryo sonradan kalsa bile O ANA KADAR kanıtlanan kurallar raporda KALIR:
/// "3. adım patladı" bilgisi 1. adımın kapıyı doğruladığını silmez.
#[tokio::test]
async fn proved_rules_survive_a_later_failure() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        analyst_approve(true),                 // kapı tutar → kanıt
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        analyst_approve(false),                // artık geçer
        ScenarioStep::Action {
            action: "boyle_bir_aksiyon_yok".into(),
            actor: Some(sc_actor("branchManager")),
            input: json!({}),
            node: None,
            target: None,
            expect_reject: false,
        },
    ];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert_eq!(res.rejected_as_expected.len(), 1, "{:?}", res.rejected_as_expected);
    assert!(res.rejected_as_expected[0].contains("basvuru_belgeleri/kimlik"));
}

// ── KASITLI HATA senaryoları (2026-08-19) ────────────────────────────────────
//
// Senaryo bir testtir; testin yarısı "yanlış olan gerçekten reddediliyor mu"dur.
// Bu bölüm senaryo şeklinin bunu İFADE EDEBİLDİĞİNİ ve motorun gerçekten reddettiğini
// kanıtlar. Editör tarafındaki karşılığı: hiçbir alan kullanıcıyı doğru değere
// ZORLAMAZ (tipli form yalnız REHBERDİR), `expectReject` ile ret beklenir.
//
// Motorun girdi sözleşmesi (`wfe_core::v22::pipeline::validate_action_input`):
//   · `input.required` yolu eksik/null → InvalidInput
//   · bildirilmemiş leaf yol → InvalidInput
//   · TİP denetimi YOKTUR — yanlış tip girdi anında reddedilmez, ctx'e yazılır ve
//     etkisi KARAR anında görülür (bkz. wrong_type testleri).

/// Aksiyonda bildirilmemiş bir yol gönderilirse motor reddeder.
#[tokio::test]
async fn undeclared_input_path_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "analyst_approve".into(),
            actor: Some(sc_actor("creditAnalyst")),
            // `uydurma_alan` aksiyonun input tanımında YOK.
            input: json!({ "credit_info": { "amount_requested": 1 }, "uydurma_alan": "x" }),
            node: None,
            target: None,
            expect_reject: true,
        },
    ];
    s.expect = Some(Expect { terminal: None, context_contains: None, active: Some(true) });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(
        res.rejected_as_expected[0].contains("uydurma_alan"),
        "{:?}",
        res.rejected_as_expected
    );
}

/// Zorunlu girdi hiç gönderilmezse motor reddeder.
#[tokio::test]
async fn missing_required_input_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "analyst_approve".into(),
            actor: Some(sc_actor("creditAnalyst")),
            input: json!({}), // credit_info.amount_requested zorunlu
            node: None,
            target: None,
            expect_reject: true,
        },
    ];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.rejected_as_expected[0].contains("zorunlu"), "{:?}", res.rejected_as_expected);
}

/// Zorunlu girdi `null` gönderilirse de reddedilir (eksik ile aynı kapı).
#[tokio::test]
async fn null_required_input_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "analyst_approve".into(),
            actor: Some(sc_actor("creditAnalyst")),
            input: json!({ "credit_info": { "amount_requested": null } }),
            node: None,
            target: None,
            expect_reject: true,
        },
    ];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.rejected_as_expected[0].contains("null"), "{:?}", res.rejected_as_expected);
}

/// Uydurma aksiyon adı — senaryo bunu YAZABİLİR (editör listeden seçmeye zorlamaz)
/// ve motor reddeder.
#[tokio::test]
async fn made_up_action_name_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Action {
        action: "boyle_bir_aksiyon_yok".into(),
        actor: Some(sc_actor("creditAnalyst")),
        input: json!({}),
        node: None,
        target: None,
        expect_reject: true,
    }];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
}

/// Yetkisiz aktör: node'un c_a'sına uymayan rol aksiyonu ALAMAZ.
#[tokio::test]
async fn ineligible_actor_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "analyst_approve".into(),
            // Analist havuzunun c_a'sı `creditAnalyst`; bu rol uymaz.
            actor: Some(sc_actor("temizlik_gorevlisi")),
            input: json!({ "credit_info": { "amount_requested": 1 } }),
            node: None,
            target: None,
            expect_reject: true,
        },
    ];
    s.expect = Some(Expect { terminal: None, context_contains: None, active: Some(true) });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
}

/// Var olmayan kol (`node`) ipucu — tekil akışta kol vermek de hatadır.
#[tokio::test]
async fn invalid_branch_hint_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Action {
        action: "analyst_approve".into(),
        actor: Some(sc_actor("creditAnalyst")),
        input: json!({ "credit_info": { "amount_requested": 1 } }),
        node: Some("boyle_bir_node_yok".into()),
        target: None,
        expect_reject: true,
    }];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    // Ret YETKİ kapısından gelir: var olmayan bir kolda claim de olamaz, dolayısıyla
    // uygunluk denetimi motorun kol kontrolünden ÖNCE cevap verir. Senaryo için sonuç
    // aynı ("bu adım reddedilir"), mesajın hangi kapıdan geldiği kullanıcıya yazılır.
    assert_eq!(res.rejected_as_expected.len(), 1, "{:?}", res.rejected_as_expected);
}

/// Katalogda OLMAYAN belge slotuna yükleme reddedilir.
#[tokio::test]
async fn unknown_attachment_slot_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![ScenarioStep::Attach {
        attach: wf_wfe::scenario::AttachStep {
            group: "olmayan_grup".into(),
            item: "olmayan_dosya".into(),
            filename: Some("x.pdf".into()),
            content_type: Some("application/pdf".into()),
            size_bytes: 10,
            expect_reject: true,
        },
    }];
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(res.rejected_as_expected[0].contains("olmayan_grup"), "{:?}", res.rejected_as_expected);
}

// ── yanlış TİP artık REDDEDİLİR (2026-08-19) ────────────────────────────────
//
// Motor bilir kişidir: bildirilen tip varsa ve değer o tipte gelmiyorsa reddi ENGINE
// verir (`wfe_core::v22::ctx_types` + `validate_action_input`). Bu iki test 2026-08-19
// öncesinde TERSİNİ belgeliyordu ("tip denetlenmez, değer ctx'e aynen yazılır");
// kural motora taşınınca beklenti de tersine döndü.

/// Sayı beklenen alana metin: adım REDDEDİLİR, `expectReject` ile senaryo geçer.
#[tokio::test]
async fn wrong_type_action_input_is_rejected() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.steps = vec![
        attach_step("basvuru_belgeleri", "kimlik", Some("application/pdf"), 10),
        attach_step("basvuru_belgeleri", "gelir_belgesi", Some("application/pdf"), 10),
        ScenarioStep::Action {
            action: "analyst_approve".into(),
            actor: Some(sc_actor("creditAnalyst")),
            // Şemada `amount_requested` number; burada METİN gönderiliyor.
            input: json!({ "credit_info": { "amount_requested": "yüz bin" } }),
            node: None,
            target: None,
            expect_reject: true,
        },
    ];
    s.expect = Some(Expect {
        terminal: None,
        context_contains: None,
        active: Some(true), // reddedildiği için akış analist havuzunda kaldı
    });
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert!(
        res.rejected_as_expected[0].contains("type_mismatch"),
        "{:?}",
        res.rejected_as_expected
    );
}

/// BAŞLANGIÇ girdisi yanlış tipteyse başlatma reddedilir — `expectStartReject` bunu
/// ifade eder. (Bu bayrak olmadan start hatası her koşulda senaryoyu kaldırıyordu,
/// yani "yanlış girdiyle başlatılamaz" senaryosu YAZILAMIYORDU.)
#[tokio::test]
async fn wrong_type_start_input_is_rejected_with_expect_start_reject() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.start_input = json!({
        "applicant": { "name": "Ayşe", "tckid": "1" },
        "credit_info": { "amount_requested": "yüz bin" } // number bekleniyor
    });
    s.expect_start_reject = true;
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
    assert_eq!(res.steps_executed, 0, "akış hiç başlamadı");
    assert!(
        res.rejected_as_expected[0].contains("Başlatma beklendiği gibi reddedildi"),
        "{:?}",
        res.rejected_as_expected
    );
}

/// `expectStartReject` ile başlatma BAŞARILI olursa senaryo kalır — kural delik demektir.
#[tokio::test]
async fn expect_start_reject_on_a_valid_start_fails_the_scenario() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.expect_start_reject = true; // oysa girdi doğru
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(!res.ok);
    assert!(res.failures[0].contains("reddedilmeliydi"), "{:?}", res.failures);
}

/// Yetkisiz başlatan da aynı bayrakla test edilir (girdi sözleşmesinden bağımsız).
#[tokio::test]
async fn ineligible_starter_is_rejected_with_expect_start_reject() {
    let (wfd, doc) = belge();
    let mut s = belge_scenario();
    s.start_actor = Some(sc_actor("temizlik_gorevlisi")); // start c_a'sı branchClerk
    s.expect_start_reject = true;
    let res = run(&engine(), &wfd, &doc, &s, None).await;
    assert!(res.ok, "{:?}", res.failures);
}
