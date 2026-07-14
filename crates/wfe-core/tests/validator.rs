//! Faz 1 validator testleri — WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf),
//! WOR-34 (context/expression/retry). Spec: runtime-semantics §1, §2b, §5, §6.

use serde_json::{json, Value};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{validate, ValidationReport};

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");

fn fixture_value() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn validate_value(v: Value) -> ValidationReport {
    let wfd = Wfd::from_value(v).expect("mutasyon parse edilebilir kalmalı");
    validate(&wfd)
}

fn has_error(report: &ValidationReport, code: &str) -> bool {
    report.errors.iter().any(|e| e.code == code)
}

#[test]
fn golden_fixture_is_valid() {
    let report = validate_value(fixture_value());
    assert!(
        report.errors.is_empty(),
        "golden fixture temiz geçmeli, hatalar: {:#?}",
        report.errors
    );
    assert!(report.warnings.is_empty(), "uyarılar: {:#?}", report.warnings);
}

// ---- §1 cross-reference ----

#[test]
fn unknown_from_node_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["from"] = json!("self__ghost");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn unknown_action_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["action"] = json!("ghost_action");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn unknown_trigger_use_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["trigger"][0]["use"] = json!("ghost_exec");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn unknown_wft_node_in_condition_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["node"] = json!("self__ghost");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn unknown_wft_terminal_in_default_is_error() {
    let mut v = fixture_value();
    v["transitions"][1]["wft"]["default"] = json!({"terminal": "terminal_ghost"});
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn unknown_escalation_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] = json!({"node": "self__ghost"});
    assert!(has_error(&validate_value(v), "cross_ref"));
}

// ---- §1 uniqueness ----

#[test]
fn duplicate_transition_id_is_error() {
    let mut v = fixture_value();
    v["transitions"][1]["id"] = json!("t_analyst_approve");
    assert!(has_error(&validate_value(v), "unique"));
}

#[test]
fn node_key_colliding_with_terminal_id_is_error() {
    let mut v = fixture_value();
    // terminal id'sini bir node key'i ile çakıştır — global namespace ihlali
    v["terminals"][0]["id"] = json!("self__branchManager");
    // terminal referanslarını da güncelle ki cross-ref hatasına düşmesin
    v["transitions"][0]["wft"]["conditions"][1]["terminal"] = json!("self__branchManager");
    v["transitions"][1]["wft"]["conditions"][0]["terminal"] = json!("self__branchManager");
    assert!(has_error(&validate_value(v), "unique"));
}

#[test]
fn terminal_ids_differing_only_by_case_is_error() {
    let mut v = fixture_value();
    // "terminal_approved" ile "Terminal_Approved" — sadece case farklı, aynı isim sayılır
    v["terminals"][1]["id"] = json!("Terminal_Approved");
    v["transitions"][1]["wft"]["default"]["terminal"] = json!("Terminal_Approved");
    let report = validate_value(v);
    assert!(has_error(&report, "unique"), "hatalar: {:#?}", report.errors);
}

// ---- §2b slug / canonical uniqueness ----

#[test]
fn node_key_not_matching_slug_is_error() {
    let mut v = fixture_value();
    let node = v["nodes"]["parent__creditDeptManager"].clone();
    v["nodes"].as_object_mut().unwrap().remove("parent__creditDeptManager");
    v["nodes"]["parent__wrongKey"] = node;
    // referansları düzelt ki sadece slug hatası kalsın
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] = json!({"node": "parent__wrongKey"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "parent__wrongKey"]);
    let report = validate_value(v);
    assert!(has_error(&report, "slug"), "hatalar: {:#?}", report.errors);
}

#[test]
fn collision_hash_suffix_is_accepted() {
    let mut v = fixture_value();
    let node = v["nodes"]["parent__creditDeptManager"].clone();
    v["nodes"].as_object_mut().unwrap().remove("parent__creditDeptManager");
    v["nodes"]["parent__creditDeptManager_a3f9"] = node;
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] =
        json!({"node": "parent__creditDeptManager_a3f9"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "parent__creditDeptManager_a3f9"]);
    let report = validate_value(v);
    assert!(!has_error(&report, "slug"), "hash'li key kabul edilmeli: {:#?}", report.errors);
}

#[test]
fn duplicate_canonical_c_a_is_error() {
    let mut v = fixture_value();
    // parent__creditDeptManager'ın c_a'sını self__branchManager ile aynı yap
    // (key'i de yeni slug'ın collision-hash'li haline getir ki yalnızca duplicate hatası kalsın)
    let ca = v["nodes"]["self__branchManager"]["c_a"].clone();
    let node = json!({"label": "Kopya", "c_a": ca});
    v["nodes"].as_object_mut().unwrap().remove("parent__creditDeptManager");
    v["nodes"]["self__branchManager_0000"] = node;
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] =
        json!({"node": "self__branchManager_0000"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "self__branchManager_0000"]);
    assert!(has_error(&validate_value(v), "duplicate_c_a"));
}

// ---- §5 graf ----

#[test]
fn unreachable_node_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__auditor"] = json!({
        "label": "Erişilemez",
        "c_a": {"c_orgu": "self", "c_r": ["auditor"]}
    });
    let report = validate_value(v);
    assert!(has_error(&report, "unreachable"), "hatalar: {:#?}", report.errors);
}

#[test]
fn escalation_edges_count_for_reachability() {
    // golden fixture'da parent__creditDeptManager'a YALNIZCA escalation üzerinden ulaşılır;
    // temiz geçmesi escalation kenarlarının BFS'e dahil olduğunu kanıtlar.
    let report = validate_value(fixture_value());
    assert!(report.errors.is_empty());
}

#[test]
fn node_without_exit_is_error() {
    let mut v = fixture_value();
    // parent__creditDeptManager'ı t_manager_decide.from'dan çıkar → çıkışsız kalır
    v["transitions"][1]["from"] = json!("self__branchManager");
    let report = validate_value(v);
    assert!(has_error(&report, "no_exit"), "hatalar: {:#?}", report.errors);
}

#[test]
fn duplicate_from_action_without_when_is_error() {
    let mut v = fixture_value();
    let mut t2 = v["transitions"][1].clone();
    t2["id"] = json!("t_manager_decide_dup");
    v["transitions"].as_array_mut().unwrap().push(t2);
    // ikisinde de when yok → belirsizlik hatası
    assert!(has_error(&validate_value(v), "ambiguous_transition"));
}

#[test]
fn duplicate_from_action_with_when_is_warning() {
    let mut v = fixture_value();
    let mut t2 = v["transitions"][1].clone();
    t2["id"] = json!("t_manager_decide_guarded");
    t2["when"] = json!("$ctx.credit_info.amount_requested > 1000");
    v["transitions"].as_array_mut().unwrap().push(t2);
    let report = validate_value(v);
    assert!(!has_error(&report, "ambiguous_transition"), "hatalar: {:#?}", report.errors);
    assert!(
        report.warnings.iter().any(|w| w.code == "ambiguous_transition"),
        "when'li çakışma uyarı olmalı"
    );
}

// ---- §6 context / expression / retry ----

#[test]
fn exec_response_namespace_is_error() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"]["wfes_effects"]["set"]["credit_score"] =
        json!("$exec.response.score");
    assert!(has_error(&validate_value(v), "exec_response"));
}

#[test]
fn invalid_zen_expression_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["when"] = json!("((broken ==");
    assert!(has_error(&validate_value(v), "zen_parse"));
}

#[test]
fn action_input_path_missing_from_context_is_error() {
    let mut v = fixture_value();
    v["actions"]["manager_decide"]["input"]["required"] = json!(["ghost_field"]);
    assert!(has_error(&validate_value(v), "input_path"));
}

#[test]
fn action_input_targeting_readonly_field_is_error() {
    let mut v = fixture_value();
    v["actions"]["manager_decide"]["input"]["optional"] = json!(["credit_score"]);
    assert!(has_error(&validate_value(v), "readonly_input"));
}

#[test]
fn effects_set_path_missing_from_context_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wfes_effects"]["set"]["ghost_field"] = json!("x");
    assert!(has_error(&validate_value(v), "effect_path"));
}

#[test]
fn wfd_all_not_alone_in_retrier_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["trigger"][0]["retry"][0]["error_equals"] =
        json!(["WFD.ALL", "WFD.Timeout"]);
    assert!(has_error(&validate_value(v), "retry_wfd_all"));
}

#[test]
fn wfd_all_not_in_last_retrier_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["trigger"][0]["retry"] = json!([
        {"error_equals": ["WFD.ALL"]},
        {"error_equals": ["WFD.Timeout"]}
    ]);
    assert!(has_error(&validate_value(v), "retry_wfd_all"));
}

#[test]
fn wft_condition_with_both_node_and_terminal_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["terminal"] = json!("terminal_approved");
    assert!(has_error(&validate_value(v), "wft_target"));
}

#[test]
fn wft_condition_with_neither_target_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]
        .as_object_mut()
        .unwrap()
        .remove("node");
    assert!(has_error(&validate_value(v), "wft_target"));
}

// ---- V1–V6 simetrik start ----

#[test]
fn start_from_unknown_node_is_error() {
    // V1
    let mut v = fixture_value();
    v["start"][0]["from"] = json!("type_branch__ghost");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn start_node_as_wft_target_is_error() {
    // V2 — bir escalation start node'unu hedeflerse ihlal
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] =
        json!({"node": "type_branch__branchClerk"});
    assert!(has_error(&validate_value(v), "start_target"));
}

#[test]
fn start_node_with_escalation_is_error() {
    // V3
    let mut v = fixture_value();
    v["nodes"]["type_branch__branchClerk"]["escalation"] = json!([
        {"after": "P1D", "wft": {"node": "self__creditAnalyst"}}
    ]);
    assert!(has_error(&validate_value(v), "start_escalation"));
}

#[test]
fn start_action_unknown_is_error() {
    // V4 (M16): start.action actions{} içinde tanımlı olmalı
    let mut v = fixture_value();
    v["start"][0]["action"] = json!("ghost_start_action");
    assert!(has_error(&validate_value(v), "start_action"));
}

#[test]
fn start_action_named_start_is_allowed() {
    // M16: "start" artık rezerve kelime DEĞİL — actions{} içinde tanımlıysa
    // start.action olarak da kullanılabilir.
    let mut v = fixture_value();
    v["actions"]["start"] = v["actions"]["create_application"].clone();
    v["start"][0]["action"] = json!("start");
    let report = validate_value(v);
    assert!(
        !has_error(&report, "start_action") && !has_error(&report, "reserved_action"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn golden_start_is_symmetric_from_action() {
    // V1/V4 pozitif: golden fixture yeni şekilde temiz geçer (M16: gerçek action adı)
    let v = fixture_value();
    assert_eq!(v["start"][0]["from"], json!("type_branch__branchClerk"));
    assert_eq!(v["start"][0]["action"], json!("create_application"));
    assert!(
        v["actions"].get("create_application").is_some(),
        "start aksiyonu actions{{}} içinde tanımlı olmalı"
    );
    assert!(v["start"][0].get("c_a").is_none(), "start artık c_a taşımamalı");
    assert!(validate_value(v).errors.is_empty());
}

#[test]
fn start_with_named_action_selects_matching_rule() {
    // M16 runtime resolution ile uyum: aynı action adı transitions'ta da kullanılmadığı
    // sürece yalnız start[].action üzerinden erişilir — validator bunu kabul eder.
    let v = fixture_value();
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}
