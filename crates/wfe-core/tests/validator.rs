//! Faz 1 validator testleri — WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf),
//! WOR-34 (context/expression/retry). Spec: runtime-semantics §1, §2b, §5, §6.

use serde_json::{json, Value};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{validate, ValidationReport};

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");
const PARALLEL_FIXTURE: &str = include_str!("fixtures/paralel-onay.json");
const ATTACHMENT_FIXTURE: &str = include_str!("fixtures/belge-onay.json");

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
    // WOR-70b: golden fixture'da `internal_notes` alanını hem iki aksiyonun OPSİYONEL
    // girdisi hem analist havuzunun escalation'ı yazıyor. Girdi gönderilmezse alan
    // null'a döner ve SLA notu kaybolur — bilinçli tasarım olabileceği için hata değil
    // UYARI. Fixture bu uyarıyı BEKLENEN tek uyarı olarak taşır (örnek değeri var:
    // kuralın gerçek bir akışta nasıl göründüğünü gösteriyor).
    let unexpected: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.code != "optional_input_nulls_other_writer")
        .collect();
    assert!(
        unexpected.is_empty(),
        "beklenmeyen uyarılar: {unexpected:#?}"
    );
    assert_eq!(
        report.warnings.len(),
        1,
        "yalnız internal_notes uyarısı beklenir: {:#?}",
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

// ---- SLA sözleşmesi: escalation + claim_timeout ----
// 2026-07-28: SLA-1/SLA-2 akışı BİTİREMEZ — `terminate` kaldırıldı, `wft` zorunlu;
// zaman aşımıyla akışı bitiren tek kural root `timeout` (SLA-3).

#[test]
fn escalation_terminate_is_rejected_as_removed() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["terminate"] = json!(true);
    assert!(has_error(
        &validate_value(v),
        "escalation_terminate_removed"
    ));
}

#[test]
fn escalation_terminate_without_wft_is_still_rejected() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]
        .as_object_mut()
        .unwrap()
        .remove("wft");
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["terminate"] = json!(true);
    let report = validate_value(v);
    // Hem kaldırılmış alan hem eksik hedef bildirilir.
    assert!(has_error(&report, "escalation_terminate_removed"));
    assert!(has_error(&report, "escalation_wft_required"));
}

#[test]
fn escalation_without_wft_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]
        .as_object_mut()
        .unwrap()
        .remove("wft");
    assert!(has_error(&validate_value(v), "escalation_wft_required"));
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

// ---- 2026-07-28: SLA hedefleri terminal OLAMAZ (sla_terminal_target) ----

#[test]
fn claim_timeout_terminal_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "terminal_rejected"});
    assert!(has_error(&validate_value(v), "sla_terminal_target"));
}

#[test]
fn escalation_terminal_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] =
        json!({"terminal": "terminal_rejected"});
    assert!(has_error(&validate_value(v), "sla_terminal_target"));
}

// SLA-2 hedefi YALNIZ `{node}` olabilir — conditions/parallel/collapse formları da yasak.

#[test]
fn escalation_conditional_target_is_error() {
    let mut v = fixture_value();
    // Hedeflerin HEPSİ node olsa bile koşullu dallanma SLA'nın kararı değildir.
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] = json!({
        "conditions": [{ "when": "$ctx.within_limit == true", "node": "self__branchManager" }],
        "default": { "node": "parent__creditDeptManager" }
    });
    assert!(has_error(&validate_value(v), "sla_target_not_node"));
}

#[test]
fn escalation_parallel_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] = json!({
        "parallel": {
            "branches": ["self__branchManager", "parent__creditDeptManager"],
            "join": { "node": "type_branch__branchClerk" }
        }
    });
    assert!(has_error(&validate_value(v), "sla_target_not_node"));
}

#[test]
fn escalation_collapse_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] =
        json!({ "collapse": { "node": "self__branchManager" } });
    assert!(has_error(&validate_value(v), "sla_target_not_node"));
}

#[test]
fn escalation_node_target_has_no_terminal_error() {
    let v = fixture_value();
    // Fixture'ın SLA-2 adımı `{"node": "self__branchManager"}` — kural ihlali yok.
    assert!(!has_error(&validate_value(v), "sla_terminal_target"));
}

// ---- 2026-07-28: SLA effects namespace kısıtı (sla_effect_namespace) ----

#[test]
fn sla_effects_action_input_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wfes_effects"]["set"]["internal_notes"] =
        json!("$action.input.note");
    assert!(has_error(&validate_value(v), "sla_effect_namespace"));
}

#[test]
fn claim_timeout_effects_exec_result_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] = json!({
        "after": "PT2H",
        "wfes_effects": { "set": { "internal_notes": "$exec.result.body.msg" } }
    });
    assert!(has_error(&validate_value(v), "sla_effect_namespace"));
}

#[test]
fn claim_timeout_effects_with_system_tokens_is_valid() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] = json!({
        "after": "PT2H",
        "wfes_effects": { "set": { "internal_notes": "$node", "analyst_approved_at": "$timestamp" } }
    });
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
    assert!(
        report.warnings.is_empty(),
        "uyarılar: {:#?}",
        report.warnings
    );
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
        report
            .errors
            .iter()
            .any(|e| e.code == "attachment_item_dup"),
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

// ---- WOR-72: join_mode / join_threshold ----------------------------------------

#[test]
fn quorum_or_join_is_valid() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("or");
    v["transitions"][0]["wft"]["parallel"]["join_threshold"] = json!(2);
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "2-of-3 quorum geçerli olmalı: {:?}",
        report.errors
    );
}

#[test]
fn quorum_or_join_without_threshold_is_valid() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("or");
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "eşiksiz OR = 1-of-N, geçerli: {:?}",
        report.errors
    );
}

#[test]
fn join_threshold_without_or_mode_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_threshold"] = json!(2);
    assert!(has_error(
        &validate_value(v),
        "parallel_join_threshold"
    ));
}

/// K = kol sayısı matematiksel olarak AND'dir; aynı davranışın ikinci yazımı
/// olmasın diye reddedilir (tek temsil kuralı).
#[test]
fn join_threshold_equal_to_branch_count_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("or");
    v["transitions"][0]["wft"]["parallel"]["join_threshold"] = json!(3);
    assert!(has_error(&validate_value(v), "parallel_join_threshold"));
}

#[test]
fn join_threshold_zero_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("or");
    v["transitions"][0]["wft"]["parallel"]["join_threshold"] = json!(0);
    assert!(has_error(&validate_value(v), "parallel_join_threshold"));
}

// ---- WOR-73: join_mode: expr + join_when -------------------------------------

const JOIN_EXPR: &str =
    "($branches.self__financeApprover and $branches.self__legalApprover) or $branches.self__hrApprover";

#[test]
fn expr_join_with_valid_when_is_valid() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    v["transitions"][0]["wft"]["parallel"]["join_when"] = json!(JOIN_EXPR);
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "geçerli ZEN join koşulu kabul edilmeli: {:?}",
        report.errors
    );
}

#[test]
fn expr_join_without_when_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    assert!(has_error(&validate_value(v), "parallel_join_when"));
}

#[test]
fn join_when_without_expr_mode_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_when"] = json!(JOIN_EXPR);
    assert!(has_error(&validate_value(v), "parallel_join_when"));
}

#[test]
fn expr_join_with_threshold_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    v["transitions"][0]["wft"]["parallel"]["join_when"] = json!(JOIN_EXPR);
    v["transitions"][0]["wft"]["parallel"]["join_threshold"] = json!(2);
    assert!(has_error(&validate_value(v), "parallel_join_threshold"));
}

#[test]
fn expr_join_with_unparsable_when_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    v["transitions"][0]["wft"]["parallel"]["join_when"] = json!("$branches.a and and");
    assert!(has_error(&validate_value(v), "parallel_join_when"));
}

/// Yazım hatası SESSİZ kalmamalı: `$branches.yanlisKol` runtime'da `false` döner ve
/// join hiç dolmaz — statik olarak yakalanır.
#[test]
fn expr_join_referencing_unknown_branch_is_error() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    v["transitions"][0]["wft"]["parallel"]["join_when"] =
        json!("$branches.self__financeApprover or $branches.self__yokBoyleKol");
    assert!(has_error(
        &validate_value(v),
        "parallel_join_when_unknown_branch"
    ));
}

/// `len($arrived) >= 2` de geçerli bir ifadedir (kol referansı içermez).
#[test]
fn expr_join_with_arrived_count_is_valid() {
    let mut v = parallel_fixture_value();
    v["transitions"][0]["wft"]["parallel"]["join_mode"] = json!("expr");
    v["transitions"][0]["wft"]["parallel"]["join_when"] = json!("len($arrived) >= 2");
    assert!(validate_value(v).errors.is_empty());
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

// ---- WOR-70: context yazma sözleşmesi ----

#[test]
fn attachment_and_parallel_fixtures_are_valid() {
    // Üç fixture de yeni sözleşmeye (context.required yok, her alan yazılıyor,
    // her input tüketiliyor) uymalı.
    for (name, raw) in [
        ("belge-onay", ATTACHMENT_FIXTURE),
        ("paralel-onay", PARALLEL_FIXTURE),
    ] {
        let wfd = Wfd::from_json(raw).expect("fixture parse etmeli");
        let report = validate(&wfd);
        assert!(
            report.is_valid(),
            "{name} geçerli olmalı, hatalar: {:#?}",
            report.errors
        );
    }
}

#[test]
fn root_context_required_is_error() {
    let mut v = fixture_value();
    v["context"]["required"] = json!(["applicant"]);
    assert!(has_error(&validate_value(v), "context_required_removed"));
}

#[test]
fn nested_context_required_is_error() {
    let mut v = fixture_value();
    v["context"]["properties"]["applicant"]["required"] = json!(["name"]);
    assert!(has_error(&validate_value(v), "context_required_removed"));
}

#[test]
fn context_field_no_effect_writes_is_error() {
    let mut v = fixture_value();
    // Hiçbir wfes_effects'in yazmadığı yeni bir alan — hiç dolmayacağı için reddedilir.
    v["context"]["properties"]["hayalet_alan"] = json!({"type": "string"});
    let report = validate_value(v);
    assert!(
        has_error(&report, "context_field_never_written"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn nested_context_leaf_no_effect_writes_is_error() {
    let mut v = fixture_value();
    // `applicant` bütün olarak yazılıyor ($action.input.applicant) — ata kapsaması
    // geçerli olmalı; ama HİÇ yazılmayan bir kökün yaprağı yakalanmalı.
    v["context"]["properties"]["ek_bilgi"] = json!({
        "type": "object",
        "properties": { "kanal": { "type": "string" } }
    });
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(
            |e| e.code == "context_field_never_written" && e.message.contains("ek_bilgi.kanal")
        ),
        "yaprak yol raporlanmalı, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn whole_object_effect_covers_nested_leaves() {
    // Golden'da `applicant` tek parça yazılıyor; name/tckid/income yaprakları
    // ölü sayılmamalı.
    let report = validate(&Wfd::from_json(FIXTURE).unwrap());
    assert!(
        !report
            .errors
            .iter()
            .any(|e| e.code == "context_field_never_written"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn declared_input_not_consumed_by_effects_is_error() {
    let mut v = fixture_value();
    // manager_decide'ın manager_decision yazımını sil — istekten gelen değer
    // hiçbir yere yazılmıyor.
    v["transitions"][1]["wfes_effects"]["set"]
        .as_object_mut()
        .unwrap()
        .remove("manager_decision");
    let report = validate_value(v);
    assert!(
        has_error(&report, "unused_action_input"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn optional_input_must_also_be_consumed() {
    let mut v = fixture_value();
    v["transitions"][0]["wfes_effects"]["set"]
        .as_object_mut()
        .unwrap()
        .remove("internal_notes");
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "unused_action_input" && e.message.contains("internal_notes")),
        "opsiyonel input da tüketilmeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn ancestor_input_ref_covers_dotted_declaration() {
    let mut v = fixture_value();
    // analyst_approve `credit_info.amount_requested` istiyor; effect bütün objeyi
    // yazarsa (ata referans) bu da tüketim sayılır.
    v["transitions"][0]["wfes_effects"]["set"]
        .as_object_mut()
        .unwrap()
        .remove("credit_info.amount_requested");
    v["transitions"][0]["wfes_effects"]["set"]["credit_info"] = json!("$action.input.credit_info");
    let report = validate_value(v);
    assert!(
        !report
            .errors
            .iter()
            .any(|e| e.code == "unused_action_input" && e.message.contains("credit_info")),
        "ata referans tüketim saymalı, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn autoexec_effects_count_as_input_consumers() {
    let mut v = fixture_value();
    // manager_decision'ı transition effects'ten çıkar, tetiklenen autoexec'e taşı.
    v["transitions"][1]["wfes_effects"]["set"]
        .as_object_mut()
        .unwrap()
        .remove("manager_decision");
    v["autoexec"]["audit_log"]["wfes_effects"] =
        json!({ "set": { "manager_decision": "$action.input.manager_decision" } });
    let report = validate_value(v);
    assert!(
        !has_error(&report, "unused_action_input"),
        "tetiklenen autoexec'in effects'i de tüketici sayılmalı, hatalar: {:#?}",
        report.errors
    );
}

// ---- WOR-70b: required non-null + opsiyonel girdinin null'lama uyarısı ----

#[test]
fn optional_input_with_no_other_writer_is_not_warned() {
    let mut v = fixture_value();
    // internal_notes'u yalnız manager_decide yazsın: analist transition'ından ve
    // escalation'dan kaldır → tek yazar kalır, uyarı üretilmemeli.
    v["transitions"][0]["wfes_effects"]["set"]
        .as_object_mut()
        .unwrap()
        .remove("internal_notes");
    v["actions"]["analyst_approve"]["input"]["optional"] = json!([]);
    v["nodes"]["self__creditAnalyst"]["escalation"][0]
        .as_object_mut()
        .unwrap()
        .remove("wfes_effects");
    let report = validate_value(v);
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.code == "optional_input_nulls_other_writer"),
        "tek yazar varsa uyarı olmamalı: {:#?}",
        report.warnings
    );
}

#[test]
fn required_sourced_input_does_not_trigger_the_warning() {
    let mut v = fixture_value();
    // manager_decision yalnız `required` — gönderilmesi garanti, null'a dönmez.
    // Aynı alanı bir terminal effect'i de yazsın: uyarı ÜRETİLMEMELİ.
    v["terminals"][0]["wfes_effects"] = json!({ "set": { "manager_decision": "approve" } });
    let report = validate_value(v);
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.code == "optional_input_nulls_other_writer"
                && w.message.contains("manager_decision")),
        "zorunlu girdi kaynaklı yazar uyarı üretmemeli: {:#?}",
        report.warnings
    );
}

// ---- İLK-MATCH kardeşleri: opsiyonel-girdi uyarısı yanlış pozitif vermez ----

/// Aynı (node, action) için iki `when`'li transition runtime'da İLK-MATCH ile
/// seçilir — yalnız BİRİ koşar. Aynı alanı yazsalar bile birbirinin değerini
/// ezemezler; `optional_input_nulls_other_writer` bunları eşleştirmemeli.
///
/// Regresyon: eşleştiriyordu ve iki site aynı aksiyon adını taşıdığı için mesaj
/// "'X' aksiyonu yazıyor — aynı alanı 'X' aksiyonu da yazıyor" gibi kendi kendini
/// gösteren bir cümleye dönüşüyordu.
#[test]
fn first_match_siblings_do_not_warn_about_each_other() {
    let mut v = fixture_value();
    let tx = v["transitions"].as_array_mut().unwrap();
    // Golden'daki müdür transition'ını kopyalayıp `when`siz bir kardeş ekle.
    let manager = tx
        .iter()
        .find(|t| t["action"] == "manager_decide")
        .expect("golden'da manager_decide var")
        .clone();
    let mut sibling = manager.clone();
    sibling["id"] = json!("t_manager_decide_sibling");
    sibling.as_object_mut().unwrap().remove("when");
    tx.push(sibling);

    let report = validate_value(v);
    for w in report
        .warnings
        .iter()
        .filter(|w| w.code == "optional_input_nulls_other_writer")
    {
        // 1. Yazar KENDİSİNİ "diğer yazar" olarak listelemez. `path` yazarın
        //    etiketidir; mesajın "aynı alanı ..." kısmında geçmemeli.
        let others = w
            .message
            .split_once("aynı alanı ")
            .map(|(_, rest)| rest)
            .unwrap_or("");
        assert!(
            !others.contains(&w.path),
            "yazar kendisini diğer yazar olarak listeliyor:\n  yazar: {}\n  mesaj: {}",
            w.path,
            w.message
        );
        // 2. Aynı etiket iki kez listelenmez (ilk-match kardeşleri aynı etiketi taşır).
        let labels: Vec<&str> = others
            .split(" da yazıyor")
            .next()
            .unwrap_or("")
            .split("', '")
            .collect();
        let mut uniq = labels.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(
            labels.len(),
            uniq.len(),
            "diğer-yazar listesi tekrar içeriyor: {}",
            w.message
        );
    }
}

/// Muafiyet fazla geniş olmamalı: FARKLI aksiyonlar aynı alanı yazıyorsa uyarı
/// hâlâ çıkar (golden fixture bu uyarıyı beklenen tek uyarı olarak taşır).
#[test]
fn different_writers_still_warn() {
    let report = validate_value(fixture_value());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "optional_input_nulls_other_writer"),
        "gerçek çoklu-yazar durumu hâlâ uyarı üretmeli, uyarılar: {:#?}",
        report.warnings
    );
}
