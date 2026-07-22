//! Faz 1 validator testleri — WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf),
//! WOR-34 (context/expression/retry). Spec: runtime-semantics §1, §2b, §5, §6.

use serde_json::{json, Value};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{validate, ValidationReport};

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");
const PARALLEL_FIXTURE: &str = include_str!("fixtures/example-wfd_paralel-onay_v2_2.json");
const ATTACHMENT_FIXTURE: &str = include_str!("fixtures/example-wfd_belge-onay_v2_2.json");

fn fixture_value() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}

fn parallel_fixture_value() -> Value {
    serde_json::from_str(PARALLEL_FIXTURE).unwrap()
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
    assert!(
        report.warnings.is_empty(),
        "uyarılar: {:#?}",
        report.warnings
    );
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
    assert!(
        has_error(&report, "unique"),
        "hatalar: {:#?}",
        report.errors
    );
}

// ---- §2b slug / canonical uniqueness ----

#[test]
fn node_key_not_matching_slug_is_error() {
    let mut v = fixture_value();
    let node = v["nodes"]["parent__creditDeptManager"].clone();
    v["nodes"]
        .as_object_mut()
        .unwrap()
        .remove("parent__creditDeptManager");
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
    v["nodes"]
        .as_object_mut()
        .unwrap()
        .remove("parent__creditDeptManager");
    v["nodes"]["parent__creditDeptManager_a3f9"] = node;
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] =
        json!({"node": "parent__creditDeptManager_a3f9"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "parent__creditDeptManager_a3f9"]);
    let report = validate_value(v);
    assert!(
        !has_error(&report, "slug"),
        "hash'li key kabul edilmeli: {:#?}",
        report.errors
    );
}

#[test]
fn duplicate_canonical_c_a_is_error() {
    let mut v = fixture_value();
    // parent__creditDeptManager'ın c_a'sını self__branchManager ile aynı yap
    // (key'i de yeni slug'ın collision-hash'li haline getir ki yalnızca duplicate hatası kalsın)
    let ca = v["nodes"]["self__branchManager"]["c_a"].clone();
    let node = json!({"label": "Kopya", "c_a": ca});
    v["nodes"]
        .as_object_mut()
        .unwrap()
        .remove("parent__creditDeptManager");
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
    assert!(
        has_error(&report, "unreachable"),
        "hatalar: {:#?}",
        report.errors
    );
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
    assert!(
        has_error(&report, "no_exit"),
        "hatalar: {:#?}",
        report.errors
    );
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
    assert!(
        !has_error(&report, "ambiguous_transition"),
        "hatalar: {:#?}",
        report.errors
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "ambiguous_transition"),
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

// ---- V1, V4, V5 simetrik start. V2/V3 kaldırıldı (2026-07-16): start node yeniden
// girilebilir — mid-flow'da normal node gibi wft hedefi ve escalation taşıyabilir. ----

#[test]
fn start_from_unknown_node_is_error() {
    // V1
    let mut v = fixture_value();
    v["start"][0]["from"] = json!("type_branch__ghost");
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn start_node_as_wft_target_is_allowed() {
    // eski V2: bir escalation start node'unu hedeflerse artık geçerli konfigürasyon
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] =
        json!({"node": "type_branch__branchClerk"});
    let report = validate_value(v);
    assert!(
        !has_error(&report, "start_target"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn start_node_with_escalation_is_allowed() {
    // eski V3: start node artık escalation taşıyabilir (mid-flow'da normal node gibi)
    let mut v = fixture_value();
    v["nodes"]["type_branch__branchClerk"]["escalation"] = json!([
        {"after": "P1D", "wft": {"node": "self__creditAnalyst"}}
    ]);
    let report = validate_value(v);
    assert!(
        !has_error(&report, "start_escalation"),
        "hatalar: {:#?}",
        report.errors
    );
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
    assert!(
        v["start"][0].get("c_a").is_none(),
        "start artık c_a taşımamalı"
    );
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

// ---- 2026-07-16 SLA sözleşmesi: escalation wft XOR terminate + claim_timeout ----

#[test]
fn escalation_with_both_wft_and_terminate_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["terminate"] = json!(true);
    // wft zaten fixture'da mevcut — ikisi birden XOR ihlali
    assert!(has_error(&validate_value(v), "escalation_xor"));
}

#[test]
fn escalation_with_neither_wft_nor_terminate_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]
        .as_object_mut()
        .unwrap()
        .remove("wft");
    assert!(has_error(&validate_value(v), "escalation_xor"));
}

#[test]
fn escalation_terminate_true_without_wft_is_valid() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]
        .as_object_mut()
        .unwrap()
        .remove("wft");
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["terminate"] = json!(true);
    let report = validate_value(v);
    assert!(
        !has_error(&report, "escalation_xor"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn claim_timeout_invalid_duration_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] = json!({"after": "not-a-duration"});
    assert!(has_error(&validate_value(v), "duration_format"));
}

#[test]
fn claim_timeout_unknown_wft_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "self__ghost"});
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn claim_timeout_valid_node_target_passes() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "self__branchManager"});
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn claim_timeout_without_wft_returns_to_same_pool_and_is_valid() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] = json!({"after": "PT2H"});
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

// ---- WOR-31: Parallel fork/join ----

#[test]
fn parallel_fixture_is_valid() {
    let report = validate_value(parallel_fixture_value());
    assert!(
        report.errors.is_empty(),
        "paralel-onay fixture temiz geçmeli, hatalar: {:#?}",
        report.errors
    );
    assert!(
        report.warnings.is_empty(),
        "uyarılar: {:#?}",
        report.warnings
    );
}

#[test]
fn attachment_fixture_is_valid() {
    let report = validate_value(serde_json::from_str(ATTACHMENT_FIXTURE).unwrap());
    assert!(
        report.errors.is_empty(),
        "belge-onay fixture temiz geçmeli, hatalar: {:#?}",
        report.errors
    );
    assert!(report.warnings.is_empty(), "uyarılar: {:#?}", report.warnings);
}

#[test]
fn attachment_ref_to_unknown_group_is_error() {
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["nodes"]["self__creditAnalyst"]["attachments"] = json!(["olmayan_grup"]);
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(|e| e.code == "attachment_ref"),
        "bilinmeyen attachment grubu referansı hata vermeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn attachment_duplicate_item_id_is_error() {
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["attachments"]["basvuru_belgeleri"]["items"] = json!([
        { "id": "kimlik", "required": true },
        { "id": "kimlik", "required": false }
    ]);
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(|e| e.code == "attachment_item_dup"),
        "grup içi tekrar eden item id hata vermeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn parallel_in_start_wft_is_error() {
    let mut v = parallel_fixture_value();
    v["start"][0]["wft"] = json!({
        "parallel": {
            "branches": ["self__financeApprover", "self__legalApprover"],
            "join": {"node": "self__resultCoordinator"}
        }
    });
    assert!(has_error(&validate_value(v), "parallel_start"));
}

#[test]
fn parallel_branches_below_two_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["branches"] = json!(["self__financeApprover"]);
    assert!(has_error(&validate_value(v), "parallel_branches"));
}

#[test]
fn parallel_branches_not_distinct_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["branches"] = json!([
        "self__financeApprover",
        "self__financeApprover",
        "self__hrApprover"
    ]);
    assert!(has_error(&validate_value(v), "parallel_branches"));
}

#[test]
fn parallel_join_equal_to_branch_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join"] = json!({"node": "self__financeApprover"});
    assert!(has_error(&validate_value(v), "parallel_join"));
}

#[test]
fn parallel_unknown_branch_node_is_error() {
    // branch/join hedeflerinin var olması generic cross_ref (wft_targets) ile denetlenir.
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["branches"] =
        json!(["self__ghost", "self__legalApprover", "self__hrApprover"]);
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn parallel_unknown_join_target_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join"] = json!({"node": "self__ghost"});
    assert!(has_error(&validate_value(v), "cross_ref"));
}

#[test]
fn parallel_nested_fork_is_error() {
    let mut v = parallel_fixture_value();
    // finans kolunun approve transition'ını, join yerine ikinci bir fork'a çözülecek şekilde
    // değiştir — branch subgraph içinde nested Parallel yasak.
    v["transitions"][1]["wft"] = json!({
        "parallel": {
            "branches": ["self__legalApprover", "self__hrApprover"],
            "join": {"node": "self__resultCoordinator"}
        }
    });
    assert!(has_error(&validate_value(v), "parallel_nested"));
}

#[test]
fn parallel_overlapping_branch_subgraphs_is_error() {
    let mut v = parallel_fixture_value();
    // finans kolunun approve'unu hukuk kolunun node'una yönlendir — iki branch subgraph'ı
    // artık aynı node'u paylaşıyor (join hariç ayrık olmalı kuralını çiğner).
    v["transitions"][1]["wft"] = json!({"node": "self__legalApprover"});
    assert!(has_error(&validate_value(v), "parallel_disjoint"));
}

#[test]
fn parallel_branch_dead_end_is_error() {
    let mut v = parallel_fixture_value();
    // finans kolunun approve transition'ını sil — o kol artık join'e de terminal'e de
    // ulaşamıyor (yalnız reject kalıyor, o da başka bir hatayla karışmasın diye node'u
    // kendine döndürüyoruz: gerçek dead-end üretmek için approve transition'ı kaldırıyoruz).
    v["transitions"].as_array_mut().unwrap().remove(1); // t_finance_approve
                                                        // reject de kaldırılırsa no_exit hatasına düşer; onu sadece terminal'e değil
                                                        // kendi node'una yönlendirerek dead-end'i saf tutuyoruz.
    v["transitions"][1]["wft"] = json!({"node": "self__financeApprover"});
    let report = validate_value(v);
    assert!(
        has_error(&report, "parallel_dead_end"),
        "hatalar: {:#?}",
        report.errors
    );
}
