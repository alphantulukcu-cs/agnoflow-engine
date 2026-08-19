//! Faz 1 validator testleri — WOR-32 (cross-ref, slug/uniqueness), WOR-33 (graf),
//! WOR-34 (context/expression/retry). Spec: runtime-semantics §1, §2b, §5, §6.

use serde_json::{json, Value};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{expression_issues, validate, ValidationReport};

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

// ---- GLB (global aksiyon) — `wft: {targets}` ----
//
// Hedef artık aksiyon ANAHTARINA kodlanmıyor (`__gt__` kalktı); tek transition,
// tek aksiyon, hedefi çalışma anında kişi seçiyor. Aşağıdaki kurallar o menünün
// tasarım-zamanı denetimidir.

/// `t_manager_decide`ın wft'sini verilen hedef listesiyle GLB'ye çevirir.
/// `from` iki node taşır (`self__branchManager`, `parent__creditDeptManager`) —
/// `global_action_target_self` kuralı için de elverişli.
fn with_global_targets(targets: Value) -> Value {
    let mut v = fixture_value();
    v["transitions"][1]["wft"] = json!({ "targets": targets });
    // Bu transition `terminal_rejected`a giden TEK yoldu; wft'si GLB menüsüne
    // dönünce o terminal yetim kalır ve `unreachable` GLB ile ilgisiz bir hata
    // olarak testleri kirletir. Belgeyi tutarlı bırakmak için terminali de
    // düşürüyoruz — GLB hedefleri node'dur, terminal hedefleyemez.
    if let Some(terminals) = v["terminals"].as_array_mut() {
        terminals.retain(|t| t["id"] != json!("terminal_rejected"));
    }
    v
}

#[test]
fn global_action_targets_are_valid_edges() {
    let report = with_global_targets(json!([{"node": "self__creditAnalyst"}]));
    let report = validate_value(report);
    assert!(
        report.errors.is_empty(),
        "geçerli GLB temiz geçmeli: {:#?}",
        report.errors
    );
}

#[test]
fn global_action_with_no_targets_is_error() {
    let report = validate_value(with_global_targets(json!([])));
    assert!(
        has_error(&report, "global_action_no_targets"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn global_action_unknown_target_is_error() {
    let report = validate_value(with_global_targets(json!([{"node": "self__yok"}])));
    assert!(
        has_error(&report, "global_action_target_unknown"),
        "hatalar: {:#?}",
        report.errors
    );
    // Aynı sorun ikinci kez jenerik `cross_ref` olarak BASILMAZ.
    assert!(!has_error(&report, "cross_ref"), "{:#?}", report.errors);
}

#[test]
fn duplicate_global_action_target_is_error() {
    let report = validate_value(with_global_targets(
        json!([{"node": "self__creditAnalyst"}, {"node": "self__creditAnalyst"}]),
    ));
    assert!(
        has_error(&report, "global_action_target_dup"),
        "hatalar: {:#?}",
        report.errors
    );
}

/// Kendine dönen hedef sessiz bir tuzaktır: aksiyon uygulanır, WFE aynı node'da
/// kalır, yalnız claim düşer.
#[test]
fn global_action_target_pointing_at_its_own_from_node_is_error() {
    let report = validate_value(with_global_targets(json!([{"node": "self__branchManager"}])));
    assert!(
        has_error(&report, "global_action_target_self"),
        "hatalar: {:#?}",
        report.errors
    );
}

/// Start kuralında hedefi seçecek bir aktör yoktur — kapı yayında, runtime'da değil.
#[test]
fn global_action_outside_a_transition_is_error() {
    let mut v = fixture_value();
    v["start"][0]["wft"] = json!({ "targets": [{"node": "self__branchManager"}] });
    let report = validate_value(v);
    assert!(
        has_error(&report, "global_action_placement"),
        "hatalar: {:#?}",
        report.errors
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

/// Terminal id'si artık MAKİNE kimliğidir: yalnız case farkı iki AYRI kimliktir
/// (node key'lerdeki kural). Kullanıcı metni `label` alanına yazılır.
#[test]
fn terminal_ids_differing_only_by_case_are_two_distinct_ids() {
    let mut v = fixture_value();
    v["terminals"][1]["id"] = json!("Terminal_Approved");
    v["terminals"][1]["label"] = json!("Onaylandı (büyük harfli)");
    v["transitions"][1]["wft"]["default"]["terminal"] = json!("Terminal_Approved");
    let report = validate_value(v);
    assert!(
        !has_error(&report, "terminal_id_dup"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn terminal_id_outside_the_pattern_is_error() {
    let mut v = fixture_value();
    v["terminals"][1]["id"] = json!("Onaylandı!");
    v["transitions"][1]["wft"]["default"]["terminal"] = json!("Onaylandı!");
    let report = validate_value(v);
    assert!(
        has_error(&report, "terminal_id_pattern"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn duplicate_terminal_id_is_error() {
    let mut v = fixture_value();
    v["terminals"][1]["id"] = json!("terminal_approved");
    v["transitions"][1]["wft"]["default"]["terminal"] = json!("terminal_approved");
    let report = validate_value(v);
    assert!(
        has_error(&report, "terminal_id_dup"),
        "hatalar: {:#?}",
        report.errors
    );
}

// ---- §2b slug / canonical uniqueness ----

// §2b KALDIRILDI (2026-08-12): node kimliğini tasarımcı verir. Aşağıdaki üç test —
// `node_key_not_matching_slug_is_error`, `collision_hash_suffix_is_accepted`,
// `duplicate_canonical_c_a_is_error` — kaldırılan kuralı doğruluyordu. Yerlerine
// kuralın GERÇEKTEN kalktığını doğrulayan testler kondu; sessizce silmek, kısıtın
// yanlışlıkla geri gelmesini fark ettirmezdi.

#[test]
fn node_key_need_not_match_slug_of_its_c_a() {
    let mut v = fixture_value();
    let node = v["nodes"]["parent__creditDeptManager"].clone();
    v["nodes"]
        .as_object_mut()
        .unwrap()
        .remove("parent__creditDeptManager");
    // Tasarımcının verdiği, c_a ile HİÇ ilgisi olmayan bir kimlik.
    v["nodes"]["nihai_onay"] = node;
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] = json!({"node": "nihai_onay"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "nihai_onay"]);
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "serbest node kimliği kabul edilmeli: {:#?}",
        report.errors
    );
}

#[test]
fn two_nodes_sharing_the_same_c_a_is_an_error() {
    // 2026-08-14: "aynı c_a = aynı kimlik" GERİ GELDİ, HATA seviyesinde. "Müdür inceler"
    // + "müdür onaylar" aynı havuzsa TEK node'dur; fark aksiyonların `when`i ($wfah) ile
    // verilir. 2026-08-12'de bu kural `shared_c_a` UYARISINA çevrilmişti — geri alındı.
    // Kimliği yine tasarımcı verir; geri gelen tek şey TEKİLLİK kısıtıdır.
    let mut v = fixture_value();
    let ca = v["nodes"]["self__branchManager"]["c_a"].clone();
    v["nodes"]
        .as_object_mut()
        .unwrap()
        .remove("parent__creditDeptManager");
    v["nodes"]["ikinci_inceleme"] = json!({"label": "İkinci İnceleme", "c_a": ca});
    v["nodes"]["self__branchManager"]["escalation"][0]["wft"] = json!({"node": "ikinci_inceleme"});
    v["transitions"][1]["from"] = json!(["self__branchManager", "ikinci_inceleme"]);
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(|e| e.code == "duplicate_c_a"),
        "aynı c_a HATA üretmeli: {:#?}",
        report.errors
    );
    // ...ve HATA olduğu için yayını gerçekten durdurmalı.
    assert!(
        !report.is_valid(),
        "duplicate_c_a yayını durdurmalı: {report:#?}"
    );
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

/// EDİTÖR SÖZLEŞMESİ: `POST /wfd/validate-expression` bu fonksiyonu doğrudan sunar.
/// Kurucunun serbest ZEN kutusu buradan cevap alır — WFD validator'ıyla aynı liste.
#[test]
fn expression_issues_matches_wfd_validator_verdicts() {
    // Geçerli formlar temiz.
    for ok in [
        r#"count($wfah, #.action == "x") >= 1"#,
        r#"all($wfah, #.actor.role != "x")"#,
        r#"$prev.action == "x""#,
        "$ctx.tutar > 1000",
    ] {
        assert!(expression_issues(ok).is_empty(), "temiz olmalı: {ok}");
    }

    // Parse hatası TEK hata döner (diğer kontroller anlamsız).
    for broken in [
        r#"every($wfah, #.action == "x")"#,
        r#"count(filter($wfah, #.action == "x")) >= 1"#,
        "((bozuk ==",
    ] {
        let issues = expression_issues(broken);
        assert_eq!(issues.len(), 1, "{broken}: {issues:?}");
        assert_eq!(issues[0].0, "zen_parse");
        assert!(issues[0].1, "parse hatası HATA olmalı");
    }

    // Negatif indeks: parse geçer, ayrı HATA.
    let neg = expression_issues(r#"$wfah[-1].action == "x""#);
    assert!(neg.iter().any(|(c, e, _)| *c == "zen_negative_index" && *e));

    // Korumasız indeksleme: UYARI (yayını engellemez).
    let unguarded = expression_issues(r#"$wfah[len($wfah) - 1].action == "x""#);
    assert!(unguarded
        .iter()
        .any(|(c, e, _)| *c == "wfah_index_unguarded" && !*e));
    assert!(
        !unguarded.iter().any(|(_, e, _)| *e),
        "korumasız indeksleme tek başına HATA değil: {unguarded:?}"
    );
}

/// WOR-84: negatif indeks parse edilir ama runtime'da patlar → parse kapısı yetmez.
#[test]
fn negative_index_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["when"] = json!("$wfah[-1].action == \"x\"");
    let report = validate_value(v);
    assert!(!has_error(&report, "zen_parse"), "sözdizimi geçerli");
    assert!(has_error(&report, "zen_negative_index"));
}

/// WOR-84: `$wfah` doğrudan indekslemek uyarı alır — `$prev`/`$first` patlamaz.
#[test]
fn direct_wfah_indexing_is_warned() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["when"] =
        json!("$wfah[len($wfah) - 1].action == \"x\"");
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hata DEĞİL: {:#?}", report.errors);
    assert!(report
        .warnings
        .iter()
        .any(|w| w.code == "wfah_index_unguarded"));
}

/// `$prev` kullanan aynı koşul TEMİZ geçer — önerilen yol uyarı üretmemeli.
#[test]
fn prev_namespace_produces_no_warning() {
    let mut v = fixture_value();
    v["transitions"][0]["wft"]["conditions"][0]["when"] = json!("$prev.action == \"x\"");
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
    assert!(!report
        .warnings
        .iter()
        .any(|w| w.code == "wfah_index_unguarded"));
}

/// WOR-84: calc ifadeleri artık upload kapısından geçiyor. `count(filter(...))` ve
/// `every(...)` zen'de YOK — eskiden yayınlanıp runtime'da patlıyorlardı.
#[test]
fn broken_calc_expression_is_error() {
    for broken in [
        "count(filter($wfah, #.action == \"x\")) >= 1",
        "every($wfah, #.action == \"x\")",
        "((broken ==",
    ] {
        let mut v = fixture_value();
        v["autoexec"]["kredi_skoru_getir"] = json!({
            "type": "calc",
            "config": { "expressions": { "bozuk": broken } }
        });
        assert!(
            has_error(&validate_value(v), "zen_parse"),
            "yakalanmalı: {broken}"
        );
    }
}

/// Doğru 2-argümanlı formlar temiz geçer.
#[test]
fn valid_calc_expressions_pass() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"] = json!({
        "type": "calc",
        "config": { "expressions": {
            "sayi": "count($wfah, #.action == \"x\") >= 1",
            "hepsi": "all($wfah, #.actor.role != \"x\")",
            "tam_bir": "one($wfah, #.action == \"x\")",
            "onceki": "$prev.action",
            "uzunluk": "len(filter($wfah, #.action == \"x\"))",
        }}
    });
    let report = validate_value(v);
    assert!(
        !has_error(&report, "zen_parse"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn calc_without_expressions_is_error() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"] = json!({ "type": "calc", "config": { "url": "x" } });
    assert!(has_error(&validate_value(v), "calc_expressions_missing"));
}

/// WOR-84: `terminal_when` motorda değerlendirilmiyor — sessizce yutmak yerine uyar.
#[test]
fn terminal_when_is_warned_as_ignored() {
    let mut v = fixture_value();
    v["terminal_when"] = json!("$ctx.credit_score >= 700");
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "eski dosya REDDEDİLMEZ, hatalar: {:#?}",
        report.errors
    );
    assert!(report
        .warnings
        .iter()
        .any(|w| w.code == "terminal_when_ignored"));
}

/// Alan yeniden serileştirmede DÜŞER — dosya bir kez kaydedilince kendiliğinden temizlenir.
#[test]
fn terminal_when_is_dropped_on_reserialize() {
    let mut v = fixture_value();
    v["terminal_when"] = json!("true");
    let wfd = Wfd::from_value(v).unwrap();
    assert_eq!(wfd.terminal_when.as_deref(), Some("true"));
    let out = serde_json::to_value(&wfd).unwrap();
    assert!(out.get("terminal_when").is_none());
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

// ---- 2026-08-03 (WOR-56/SLA-1): claim_timeout.collapses_parallel ----

#[test]
fn claim_timeout_collapse_without_wft_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "collapses_parallel": true});
    assert!(has_error(
        &validate_value(v),
        "claim_timeout_collapse_requires_wft"
    ));
}

/// Paraleli sonlandırma hedefi de yalnız NODE olabilir: collapse paralel modu
/// bitirir, AKIŞI bitirmez (bitirme SLA-3'ün işi).
#[test]
fn claim_timeout_collapse_terminal_target_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "terminal_rejected", "collapses_parallel": true});
    assert!(has_error(&validate_value(v), "sla_terminal_target"));
}

/// Fork'u olan dokümanda node hedefli collapse temiz geçer (hata + uyarı yok).
#[test]
fn claim_timeout_collapse_with_node_target_is_valid_in_parallel_wfd() {
    let mut v = parallel_fixture_value();
    v["nodes"]["self__financeApprover"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "self__coordinator", "collapses_parallel": true});
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
    assert!(
        report.warnings.is_empty(),
        "uyarılar: {:#?}",
        report.warnings
    );
}

/// 2026-08-03 — collapse YALNIZ paralel kolun içindeki node'da: dokümanda hiç fork
/// yoksa hiçbir node kol içinde değildir → HATA (eskiden uyarıydı).
#[test]
fn claim_timeout_collapse_without_any_fork_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "self__branchManager", "collapses_parallel": true});
    assert!(has_error(
        &validate_value(v),
        "claim_timeout_collapse_outside_parallel"
    ));
}

/// Fork VAR ama node kolun DIŞINDA (join sonrası koordinatör) → HATA. Paralel akışa
/// bağlı olmayan bir node'un süresi dolduğunda düşürülecek kardeş kol yoktur.
#[test]
fn claim_timeout_collapse_on_node_outside_branch_is_error() {
    let mut v = parallel_fixture_value();
    v["nodes"]["self__coordinator"]["claim_timeout"] =
        json!({"after": "PT2H", "wft": "self__financeApprover", "collapses_parallel": true});
    assert!(has_error(
        &validate_value(v),
        "claim_timeout_collapse_outside_parallel"
    ));
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

// ---- 2026-08-03 (WOR-56/SLA-2): node hedefli collapse ARTIK GEÇERLİ ----

/// Fork'u olan dokümanda SLA-2 `{collapse:{node}}` temiz geçer: "kimse süresinde
/// bakmadıysa paraleli kapat, işi şu gruba götür" bir dallanma kararı değildir.
#[test]
fn escalation_collapse_node_target_is_valid_in_parallel_wfd() {
    let mut v = parallel_fixture_value();
    v["nodes"]["self__financeApprover"]["escalation"] = json!([{
        "after": "P1D",
        "wft": { "collapse": { "node": "self__coordinator" } }
    }]);
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
    assert!(
        report.warnings.is_empty(),
        "uyarılar: {:#?}",
        report.warnings
    );
}

/// Collapse hedefi de terminal olamaz — collapse paraleli bitirir, AKIŞI bitirmez.
#[test]
fn escalation_collapse_terminal_target_is_error() {
    let mut v = parallel_fixture_value();
    v["nodes"]["self__financeApprover"]["escalation"] = json!([{
        "after": "P1D",
        "wft": { "collapse": { "terminal": "terminal_rejected" } }
    }]);
    assert!(has_error(&validate_value(v), "sla_terminal_target"));
}

/// 2026-08-03 — collapse YALNIZ paralel kolun içindeki node'da: dokümanda hiç fork
/// yoksa HATA (eskiden uyarıydı).
#[test]
fn escalation_collapse_without_any_fork_is_error() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["escalation"][0]["wft"] =
        json!({ "collapse": { "node": "self__branchManager" } });
    assert!(has_error(
        &validate_value(v),
        "escalation_collapse_outside_parallel"
    ));
}

/// Fork VAR ama kaynak node kolun DIŞINDA → HATA.
#[test]
fn escalation_collapse_on_node_outside_branch_is_error() {
    let mut v = parallel_fixture_value();
    v["nodes"]["self__coordinator"]["escalation"] = json!([{
        "after": "P1D",
        "wft": { "collapse": { "node": "self__financeApprover" } }
    }]);
    assert!(has_error(
        &validate_value(v),
        "escalation_collapse_outside_parallel"
    ));
}

/// Kol GİRİŞİ olmayan ama kolun İÇİNDE kalan node da collapse edebilir: interior
/// kümesi kol girişinden transition'larla BFS ile derinleşir (`check_parallel`'in
/// branch subgraph yürüyüşüyle aynı). Fixture'ın kolları tek adımlık olduğu için
/// finans kolu burada `requester`'a uzatılır (o da join'e çıkar → dead-end yok).
#[test]
fn escalation_collapse_on_deep_branch_node_is_valid() {
    let mut v = parallel_fixture_value();
    let txs = v["transitions"].as_array_mut().unwrap();
    txs.push(json!({
        "id": "t_fin_revise", "from": "self__financeApprover", "action": "reject",
        "when": "$ctx.request.amount > 1000000",
        "wft": { "node": "self__requester" }
    }));
    txs.push(json!({
        "id": "t_requester_resubmit", "from": "self__requester", "action": "approve",
        "wft": { "node": "self__resultCoordinator" }
    }));
    // Kolun DERİNİNDEKİ node (`self__requester`) collapse edebilir.
    v["nodes"]["self__requester"]["escalation"] = json!([{
        "after": "P1D",
        "wft": { "collapse": { "node": "self__coordinator" } }
    }]);
    let report = validate_value(v);
    assert!(
        !has_error(&report, "escalation_collapse_outside_parallel"),
        "kol derinliğindeki node kol içi sayılmalı: {:#?}",
        report.errors
    );
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
fn attachment_scoped_ref_to_unknown_action_is_error() {
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    // `analyst_approve` bu node'dan değil, analist node'undan çıkar — kapsam hiç uygulanmaz.
    v["nodes"]["self__branchManager"]["attachments"] = json!([
        { "group": "onay_belgeleri", "actions": ["analyst_approve"] }
    ]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "attachment_action_ref"),
        "node'dan çıkmayan aksiyona kapsam hata vermeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn attachment_scoped_ref_duplicate_action_is_error() {
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["nodes"]["self__branchManager"]["attachments"] = json!([
        { "group": "onay_belgeleri", "actions": ["manager_decide", "manager_decide"] }
    ]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "attachment_action_dup"),
        "kapsamda tekrar eden aksiyon hata vermeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn attachment_scoped_ref_with_empty_actions_is_clean() {
    // `actions: []` = hiçbir aksiyonu kapamaz (opsiyonel yükleme) — geçerli bir bildirim.
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["nodes"]["self__branchManager"]["attachments"] = json!([
        { "group": "onay_belgeleri", "actions": [] }
    ]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .all(|e| !e.code.starts_with("attachment_")),
        "boş kapsam hata üretmemeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn attachment_scope_on_start_action_is_clean() {
    // Başlatma aksiyonu da belge isteyebilir; `start[].action` bir transition DEĞİLDİR,
    // yalnız transition'lara bakan bir erişilebilirlik kontrolü bunu yanlışlıkla reddederdi.
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["nodes"]["type_branch__branchClerk"]["attachments"] = json!([
        { "group": "basvuru_belgeleri", "actions": ["create_application"] }
    ]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .all(|e| !e.code.starts_with("attachment_")),
        "start aksiyonuna konan kapsam hata üretmemeli, hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn attachment_scoped_ref_duplicate_group_is_error() {
    // Kapsam biçimi tekrar denetimini atlatmamalı: aynı grup iki kez, biri scoped.
    let mut v = serde_json::from_str::<Value>(ATTACHMENT_FIXTURE).unwrap();
    v["nodes"]["self__branchManager"]["attachments"] = json!([
        "onay_belgeleri",
        { "group": "onay_belgeleri", "actions": ["manager_decide"] }
    ]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "attachment_ref_dup"),
        "aynı grup iki referansta hata vermeli, hatalar: {:#?}",
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

// ---- effect_type_mismatch ----
//
// `$actor` motorda NESNE serileşir (`effects::resolve_dollar_string` →
// `to_value(Actor)` = {orgu_id, user_id, role}). Motor yazmayı reddetmez — çalışma
// anında context şeması zorlanmaz — dolayısıyla `string` bir alana yazıldığında hata
// hiçbir yerde görünmez: o alanı okuyan koşullar sessizce hep-false olur.

#[test]
fn actor_written_into_string_field_is_error() {
    let mut v = fixture_value();
    // Golden'da `initiated_by` object'tir ve `$actor` oraya yazılır; tipi string'e
    // çevirmek yazımı uyumsuz yapar.
    v["context"]["properties"]["initiated_by"] = json!({"type": "string"});
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "effect_type_mismatch" && e.message.contains("initiated_by")),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn actor_written_into_object_field_is_clean() {
    // Golden zaten `initiated_by: {type: object}` bildiriyor — kural sessiz kalmalı.
    let report = validate(&Wfd::from_json(FIXTURE).unwrap());
    assert!(
        !report
            .errors
            .iter()
            .any(|e| e.code == "effect_type_mismatch"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn timestamp_written_into_number_field_is_error() {
    let mut v = fixture_value();
    // `analyst_approved_at` alanına `$timestamp` yazılıyor (metin); tipi number yapmak
    // uyumsuzluk üretir.
    v["context"]["properties"]["analyst_approved_at"] = json!({"type": "number"});
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "effect_type_mismatch" && e.message.contains("analyst_approved_at")),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn untyped_source_never_flags_effect_type_mismatch() {
    let mut v = fixture_value();
    // `manager_decision`ın TEK yazarı `$action.input.manager_decision`: girdi yolu ile
    // hedef alan AYNI şema düğümüdür, dolayısıyla tip her zaman uyar — hedefin tipi ne
    // olursa olsun hata üretilmemeli. (Girdi kaynaklı yazımlar 2026-08-10'dan beri
    // context şemasından TİPLENİR; kural yalnız yol ile hedef AYRIŞTIĞINDA konuşur.)
    v["context"]["properties"]["manager_decision"] = json!({"type": "boolean"});
    // `credit_grade` yalnız `$exec.result.grade` ile yazılır — o da tipsizdir.
    v["context"]["properties"]["credit_grade"] = json!({"type": "number"});
    let report = validate_value(v);
    assert!(
        !report.errors.iter().any(|e| e.code == "effect_type_mismatch"),
        "hatalar: {:#?}",
        report.errors
    );
}

/// Girdi kaynaklı yazımın tipi context şemasından okunur: yol ile hedef ayrışırsa
/// uyumsuzluk yakalanır. Gerçek vaka (`vki` WFD'si): `"user.yas": "$action.input.user"`
/// — number bir alana TÜM obje yazılıyordu ve hiçbir kural konuşmuyordu.
#[test]
fn action_input_object_written_into_scalar_field_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wfes_effects"]["set"]["credit_info.amount_requested"] =
        json!("$action.input.applicant");
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(|e| e.code == "effect_type_mismatch"
            && e.message.contains("credit_info.amount_requested")),
        "hatalar: {:#?}",
        report.errors
    );
}

/// Aynı yolu kendi alanına yazmak (olağan durum) hata ÜRETMEZ — yol ile hedef aynı şema
/// düğümüdür. Kuralın yanlış pozitif üretmediğinin çıpası.
#[test]
fn action_input_written_into_its_own_field_is_clean() {
    let mut v = fixture_value();
    v["context"]["properties"]["credit_info"]["properties"]["amount_requested"] =
        json!({"type": "number"});
    v["transitions"][0]["wfes_effects"]["set"]["credit_info.amount_requested"] =
        json!("$action.input.credit_info.amount_requested");
    let report = validate_value(v);
    assert!(
        !report.errors.iter().any(|e| e.code == "effect_type_mismatch"),
        "hatalar: {:#?}",
        report.errors
    );
}

#[test]
fn literal_effect_value_type_is_checked() {
    let mut v = fixture_value();
    // Escalation düz metin yazıyor (`internal_notes`) — sabitin tipi de denetlenir.
    v["context"]["properties"]["internal_notes"] = json!({"type": "boolean"});
    let report = validate_value(v);
    assert!(
        report.errors.iter().any(|e| e.code == "effect_type_mismatch"
            && e.path.contains("escalation")
            && e.message.contains("internal_notes")),
        "hatalar: {:#?}",
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
fn same_path_in_required_and_optional_is_error() {
    let mut v = fixture_value();
    // `credit_info.amount_requested` analyst_approve'un ZORUNLU girdisi; aynı yolu
    // optional'a da yazmak çelişkidir (null olamaz ↔ gönderilmezse null yazılır).
    v["actions"]["analyst_approve"]["input"]["optional"] =
        json!(["internal_notes", "credit_info.amount_requested"]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "input_required_and_optional"),
        "hatalar: {:#?}",
        report.errors
    );
}

/// Ata + torun aynı listede REDDEDİLMEZ: pipeline sözleşmesi null denetimini yalnız
/// bildirilen yola uygular, iç alanın da dolu olması istenirse ayrıca bildirilir.
#[test]
fn ancestor_and_leaf_in_the_same_required_list_is_allowed() {
    let mut v = fixture_value();
    v["actions"]["create_application"]["input"]["required"] =
        json!(["applicant", "credit_info", "credit_info.amount_requested"]);
    let report = validate_value(v);
    assert!(
        !has_error(&report, "input_required_and_optional"),
        "ata/torun bildirimi bu kuralın kapsamında değil: {:#?}",
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

// ---- wft_dead_condition: koşulsuz dal sonrası her şey ölü ----

/// Editörde "aynı adımdan birden fazla ok" çizilirse export bunları `when: "true"`
/// koşullarına derler; motor ilk-match uyguladığı için ikinci ve sonraki hedefler
/// ASLA çalışmaz. Sessiz bırakılırsa akış yazarı iki hedef tanımladığını sanır.
#[test]
fn unconditional_branch_before_others_is_rejected() {
    let mut v = fixture_value();
    let tx = v["transitions"].as_array_mut().unwrap();
    let t = tx
        .iter_mut()
        .find(|t| t["action"] == "manager_decide")
        .unwrap();
    t["wft"] = json!({
        "conditions": [
            { "when": "true", "terminal": "terminal_approved" },
            { "when": "$action.input.manager_decision == 'reject'", "terminal": "terminal_rejected" }
        ],
        "default": { "terminal": "terminal_rejected" }
    });
    let report = validate_value(v);
    assert!(
        has_error(&report, "wft_dead_condition"),
        "koşulsuz dal sonrası ölü koşullar raporlanmalı: {:#?}",
        report.errors
    );
}

/// Koşulsuz dal SON ve `default` yoksa sorun yok — `default` yerine geçer.
#[test]
fn trailing_unconditional_branch_is_allowed() {
    let mut v = fixture_value();
    let tx = v["transitions"].as_array_mut().unwrap();
    let t = tx
        .iter_mut()
        .find(|t| t["action"] == "manager_decide")
        .unwrap();
    t["wft"] = json!({
        "conditions": [
            { "when": "$action.input.manager_decision == 'approve'", "terminal": "terminal_approved" },
            { "when": "true", "terminal": "terminal_rejected" }
        ]
    });
    assert!(!has_error(&validate_value(v), "wft_dead_condition"));
}

/// Koşulsuz dal SON ama `default` VAR → default asla çalışmaz, bu da ölü.
#[test]
fn trailing_unconditional_branch_with_default_is_rejected() {
    let mut v = fixture_value();
    let tx = v["transitions"].as_array_mut().unwrap();
    let t = tx
        .iter_mut()
        .find(|t| t["action"] == "manager_decide")
        .unwrap();
    t["wft"] = json!({
        "conditions": [
            { "when": "$action.input.manager_decision == 'approve'", "terminal": "terminal_approved" },
            { "when": "true", "terminal": "terminal_rejected" }
        ],
        "default": { "terminal": "terminal_approved" }
    });
    assert!(has_error(&validate_value(v), "wft_dead_condition"));
}

/// Golden fixture bu kurala TAKILMAZ — gerçek koşullar kullanıyor.
#[test]
fn golden_fixture_has_no_dead_conditions() {
    assert!(!has_error(
        &validate_value(fixture_value()),
        "wft_dead_condition"
    ));
}

// ---- when ifadelerinin TİP denetimi (expr_types) ----
//
// Editörün ürettiği JSON ile ELLE yazılan JSON aynı kurallara uymak zorundadır. Bu
// kurallar önce yalnız editörde vardı; upload kapısı bakmıyordu, dolayısıyla elle
// hazırlanmış bir dosya `some($wfah, #.actor == "ali")` ile yayınlanabiliyordu.
//
// Kuralların dayanağı `zen-expression`ın VM'i (bkz. src/expr_types.rs modül dokümanı):
// `Equal` yalnız aynı tipteki skalerleri eşler, diğer her kombinasyon sessizce `false`
// (`!=` sessizce `true`); `Compare` yalnız sayı/tarih bilir, gerisi RUNTIME hatası.

/// İlk transition'ın `when`ini verilen ifadeyle değiştirir.
fn fixture_with_when(expr: &str) -> Value {
    let mut v = fixture_value();
    v["transitions"][0]["when"] = json!(expr);
    v
}

fn errors_for_when(expr: &str) -> Vec<String> {
    validate_value(fixture_with_when(expr))
        .errors
        .into_iter()
        .map(|e| e.code)
        .collect()
}

#[test]
fn wfah_object_compared_with_scalar_is_error() {
    // `#.actor` bir nesnedir ({orgu_id, user_id, role}) — metinle eşleşmesi imkânsız.
    assert!(errors_for_when(r#"some($wfah, #.actor == "ali")"#)
        .contains(&"zen_object_compare".to_string()));
}

#[test]
fn object_compared_with_object_is_error() {
    // Golden'da `initiated_by` object'tir. Obje==obje motorda DAİMA false döner
    // (`!=` daima true) — "aynı kişi mi" sorusu alt alanla sorulur.
    assert!(
        errors_for_when(r#"some($wfah, #.actor == $ctx.initiated_by)"#)
            .contains(&"zen_object_compare".to_string()),
        "obje-obje karşılaştırması reddedilmeli"
    );
}

#[test]
fn actor_subfield_comparison_is_clean() {
    let report = validate_value(fixture_with_when(r#"some($wfah, #.actor.role == "creditAnalyst")"#));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn wfah_input_type_is_inferred_from_effects() {
    // `#.input.applicant.name` TİPSİZ bildirilir (actions[].input yalnız yol listesidir),
    // ama `applicant` context'e `$action.input.applicant` ile yazılır → tipi ctx şemasından
    // bilinir: `applicant.name` string. Sağ taraf number → sessizce hep-false olurdu.
    assert!(
        errors_for_when(
            r#"some($wfah, #.action == "create_application" and #.input.applicant.name == $ctx.credit_info.amount_requested)"#
        )
        .contains(&"zen_type_mismatch".to_string()),
        "metin ↔ sayı karşılaştırması yakalanmalı"
    );
}

#[test]
fn matching_types_are_clean() {
    let report = validate_value(fixture_with_when(
        r#"some($wfah, #.action == "create_application" and #.input.applicant.name == $ctx.applicant.name)"#,
    ));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn string_literal_against_number_field_is_error() {
    assert!(
        errors_for_when(r#"$ctx.credit_info.amount_requested == "1000""#)
            .contains(&"zen_type_mismatch".to_string()),
        "sayı alanına metin sabiti yakalanmalı"
    );
}

#[test]
fn ordering_on_text_field_is_error() {
    // Metin alanda `>` zen'de RUNTIME hatası verir (Compare: Unsupported type).
    // `#.at` de metindir (`yyyyMMddHHmmss`) — sıralaması aynı kurala takılır.
    assert!(errors_for_when(r#"some($wfah, #.actor.role > "mudur")"#)
        .contains(&"zen_ordering_not_number".to_string()));
    assert!(errors_for_when(r#"some($wfah, #.at > "20260115103000")"#)
        .contains(&"zen_ordering_not_number".to_string()));
}

// `#.at` düz bir METİNDİR (`yyyyMMddHHmmss`, UTC, 14 rakam) ve karşılaştırmaları string
// temellidir. Tip kuralı bunu görmez (iki taraf da `string`), denetlenen şey SABİTİN BİÇİMİ:
// biçime uymayan değer hiçbir kayıtla eşleşmez → satır sessizce hep-false olur.
#[test]
fn timestamp_literal_format_is_checked() {
    for when in [
        r#"some($wfah, #.at == "2026-01-01")"#,
        r#"some($wfah, #.at != "onay")"#,
        r#"some($wfah, #.at == "20260115")"#, // yarım damga — eşitlik 14 hane ister
        r#"$prev.at == "bugün""#,
        r#"some($wfah, startsWith(#.at, "Ocak"))"#,
        r#"some($wfah, startsWith(#.at, "2026-01"))"#,
        r#"some($wfah, startsWith(#.at, "2026011"))"#, // yarım alan
        r#"some($wfah, contains(#.at, "-01-"))"#,
        r#"some($wfah, #.at in ["20260115103000", "2026"])"#,
    ] {
        assert!(
            errors_for_when(when).contains(&"zen_timestamp_format".to_string()),
            "yakalanmalı: {when}"
        );
    }
}

#[test]
fn timestamp_string_comparisons_are_clean() {
    for when in [
        r#"some($wfah, #.at == "20260115103000")"#,
        r#"some($wfah, startsWith(#.at, "2026"))"#,
        r#"some($wfah, startsWith(#.at, "20260115"))"#,
        r#"some($wfah, contains(#.at, "0115"))"#,
        r#"some($wfah, matches(#.at, "^2026"))"#,
        r#"some($wfah, #.at in ["20260115103000", "20260116090000"])"#,
        "some($wfah, #.at != null)",
    ] {
        let report = validate_value(fixture_with_when(when));
        assert!(
            report.errors.is_empty(),
            "temiz geçmeli: {when} — hatalar: {:#?}",
            report.errors
        );
    }
}

// Motorun `In` opcode'u öğe öğe `Equal` yapar → farklı tipli öğe HİÇ eşleşmez, koşul
// sessizce hep-false olur. `in` dalı eskiden tip denetiminden erken dönüyordu.
#[test]
fn list_element_type_mismatch_is_error() {
    assert!(errors_for_when(r#"some($wfah, #.seq in ["a", "b"])"#)
        .contains(&"zen_list_type_mismatch".to_string()));
    assert!(
        errors_for_when(r#"$ctx.credit_info.amount_requested in ["1000"]"#)
            .contains(&"zen_list_type_mismatch".to_string())
    );
}

#[test]
fn list_element_type_match_is_clean() {
    let report = validate_value(fixture_with_when("some($wfah, #.seq in [1, 2])"));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

// Metin fonksiyonu biçimli operatörler METİN ister; `type_of` fonksiyon çağrısını
// `Unknown` saydığı için bu satırlar hiçbir kurala takılmıyordu.
#[test]
fn text_op_on_number_field_is_error() {
    assert!(errors_for_when(r#"some($wfah, startsWith(#.seq, "1"))"#)
        .contains(&"zen_text_op_not_string".to_string()));
    assert!(
        errors_for_when(r#"contains($ctx.credit_info.amount_requested, "10")"#)
            .contains(&"zen_text_op_not_string".to_string())
    );
}

#[test]
fn text_op_on_string_field_is_clean() {
    let report = validate_value(fixture_with_when(
        r#"some($wfah, startsWith(#.action, "create"))"#,
    ));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn ordering_on_number_field_is_clean() {
    let report = validate_value(fixture_with_when("some($wfah, #.seq > 1)"));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn unknown_wfah_field_is_error() {
    // `actor.name` motorun izdüşümünde YOK (`{seq, action, actor, input, at}`) — koşul
    // sessizce null okur.
    assert!(errors_for_when(r#"some($wfah, #.actor.name == "ali")"#)
        .contains(&"zen_wfah_field_unknown".to_string()));
}

#[test]
fn undeclared_input_path_is_error() {
    assert!(
        errors_for_when(r#"some($wfah, #.input.boyle_bir_girdi_yok == "x")"#)
            .contains(&"zen_wfah_field_unknown".to_string()),
        "hiçbir aksiyonun bildirmediği girdi yolu reddedilmeli"
    );
}

#[test]
fn ungated_input_ordering_is_error() {
    // Girdisi olmayan bir geçmiş kaydında `null > 1000` → RUNTIME hatası (HTTP 500).
    assert!(
        errors_for_when("some($wfah, #.input.credit_info.amount_requested > 1000)")
            .contains(&"zen_input_needs_action_gate".to_string()),
        "kapısız sıralama reddedilmeli"
    );
}

#[test]
fn gated_input_ordering_is_clean() {
    let report = validate_value(fixture_with_when(
        r#"some($wfah, #.action == "analyst_approve" and #.input.credit_info.amount_requested > 1000)"#,
    ));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn or_is_not_a_gate() {
    // `or` kapı DEĞİLDİR: sol dal koşmadığında girdi garanti değildir.
    assert!(errors_for_when(
        r#"some($wfah, #.action == "analyst_approve" or #.input.credit_info.amount_requested > 1000)"#
    )
    .contains(&"zen_input_needs_action_gate".to_string()));
}

#[test]
fn null_comparison_is_clean() {
    // Varlık sorgusu (editörün "var"/"yok" operatörü) — obje alanda BİLE meşrudur.
    let report = validate_value(fixture_with_when("some($wfah, #.input == null)"));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn date_wrapped_ordering_is_exempt() {
    // `d()` sonucu Dynamic'tir ve motorun `Compare`ı (Date,Date) çiftini bilir — fonksiyon
    // sonuçları BİLİNMEZ sayıldığı için kural buraya karışmaz.
    let report = validate_value(fixture_with_when(r#"some($wfah, d(#.at) > d("2026-01-01"))"#));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

// ---- $env — ortam konfigürasyonu referansları ----

/// Editör sözleşmesi `$env`'i de kapsar: bozuk referans `POST /wfd/validate-expression`
/// üzerinden anında görünür, WFD validator'ıyla aynı koddan.
#[test]
fn expression_issues_flags_malformed_env_reference() {
    assert!(expression_issues("$env.AUTH_API == 'x'").is_empty());
    assert!(expression_issues("$env.MAX_TUTAR > 1000").is_empty());

    // Küçük harfli anahtar zen'de GEÇERLİ bir property path'tir — parse geçer,
    // yakalayan tek şey bu kuraldır.
    let issues = expression_issues("$env.auth_api == 'x'");
    assert!(
        issues
            .iter()
            .any(|(c, e, _)| *c == "env_reference_malformed" && *e),
        "{issues:?}"
    );

    // `$env.` tek başına zaten parse edilemez; parse hatası diğer kontrolleri kısa devre
    // yapar (mevcut sözleşme) — bu kural onu değiştirmez.
    let dangling = expression_issues("$env. == 'x'");
    assert_eq!(dangling.len(), 1);
    assert_eq!(dangling[0].0, "zen_parse");
}

/// Referans toplama doküman GENELİNDEDİR: autoexec config, ZEN ifadeleri, effects.
/// Alan alan gezilmediği için yeni bir alan eklendiğinde güncelleme gerekmez.
#[test]
fn env_references_collects_across_document() {
    let mut v = fixture_value();
    assert!(
        wfe_core::validator::env_references(&Wfd::from_value(v.clone()).unwrap())
            .unwrap()
            .is_empty(),
        "golden fixture $env kullanmaz"
    );

    v["autoexec"]["kredi_skoru_getir"]["config"]["url"] = json!("$env.SCORE_API/v1/score");
    v["autoexec"]["kredi_skoru_getir"]["config"]["params"] =
        json!({ "region": "$env.REGION" });
    let refs = wfe_core::validator::env_references(&Wfd::from_value(v).unwrap()).unwrap();
    assert_eq!(
        refs.into_iter().collect::<Vec<_>>(),
        vec!["REGION".to_string(), "SCORE_API".to_string()]
    );
}

/// Bozuk referans YAYINI ENGELLER — runtime'da "$env.foo" düz metin olarak URL'ye girer.
#[test]
fn malformed_env_reference_blocks_publish() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"]["config"]["url"] = json!("https://x/$env.score_api");
    assert!(has_error(&validate_value(v), "env_reference_malformed"));
}

// ---- unknown_dollar_ref — motorun DÜZ METİN yazdığı sessiz yazım hataları ----
//
// `resolve_dollar_string` tanımadığı `$`-string'i hata saymaz, metin sabiti olarak yazar
// (son satır: `Ok(Value::from(s))`). Yani `$actor.role` yazan bir effect alana
// `"$actor.role"` METNİNİ koyar; çalışma anında hata yok, log yok, o alanı okuyan
// koşullar sessizce hep-false. Tek yakalama noktası tasarım zamanıdır.

#[test]
fn unknown_dollar_ref_in_effects_is_error() {
    let mut v = fixture_value();
    // `$actor` çıplak haliyle vardır; ALT YOLU yoktur (nesnedir, effects onu bütün yazar).
    v["transitions"][0]["wfes_effects"]["set"]["internal_notes"] = json!("$actor.role");
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.code == "unknown_dollar_ref" && e.message.contains("$actor.role")),
        "hatalar: {:#?}",
        report.errors
    );
}

// ---- x-wf-kind: c_orgu anchor'ının context alanına bağlanması ----
//
// Anchor formu (`{from: "$ctx.<yol>", traverse}`) bir context alanından ORGU çözer. O alanın
// gerçekten ORGU tuttuğu tasarım-zamanında bilinmelidir: runtime'da çözülemeyen anchor'da
// KİMSE yetkilenmez, yani akış sessizce kilitlenir.
//
// Mutasyonlar `listable[0]` / `transitions[0].c_a` / `x-visibility` üzerinde yapılır —
// node `c_a`'sı değiştirilirse `duplicate_c_a` (§2b tekilliği) tetiklenebilir ve testin
// ölçtüğü şey bulanıklaşır. (Eski gerekçe "node key = slug(c_a)" değişmeziydi; o kural
// 2026-08-12'de kalktı, kaçınma sebebi değişti ama DURUYOR.)

/// Anchor'ı verilen yola bağlanmış bir `listable` kuralı kurar.
fn with_listable_anchor(from: &str) -> Value {
    let mut v = fixture_value();
    v["listable"][0]["c_a"]["c_orgu"] = json!({ "from": from, "traverse": "self" });
    v
}

/// Context şemasına `x-wf-kind` bildirilmiş bir alan ekler ve onu yazan bir effect verir
/// (WOR-70: her alanın en az bir yazarı olmalı, yoksa `context_field_never_written`).
fn with_kinded_field(mut v: Value, name: &str, kind: &str, props: Value) -> Value {
    v["context"]["properties"][name] = json!({
        "type": "object",
        "x-wf-kind": kind,
        "properties": props,
    });
    v["transitions"][0]["wfes_effects"]["set"][name] = json!("$actor");
    v
}

#[test]
fn anchor_to_orgu_kinded_field_is_valid() {
    let v = with_kinded_field(
        with_listable_anchor("$ctx.musteri_sube"),
        "musteri_sube",
        "orgu",
        json!({ "orgu_id": { "type": "string" }, "name": { "type": "string" } }),
    );
    let report = validate_value(v);
    assert!(
        !has_error(&report, "c_orgu_anchor_not_orgu_kind")
            && !has_error(&report, "c_orgu_anchor_unknown_field"),
        "orgu kind'lı alana bağlı anchor geçerli olmalı: {:#?}",
        report.errors
    );
}

/// Motor obje/dizi değerleri RECURSIVE çözer — içerideki yazım hatası da yakalanmalı.
#[test]
fn unknown_dollar_ref_inside_object_value_is_error() {
    let mut v = fixture_value();
    v["transitions"][0]["wfes_effects"]["set"]["internal_notes"] =
        json!({ "rol": "$actor.role", "ok": "$ctx.credit_score" });
    assert!(has_error(&validate_value(v), "unknown_dollar_ref"));
}

/// Autoexec config'i de aynı gramerle çözülür (`runner::resolve_config_string`).
#[test]
fn unknown_dollar_ref_in_autoexec_config_is_error() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"]["config"]["url"] = json!("$ctxx.base_url");
    assert!(has_error(&validate_value(v), "unknown_dollar_ref"));
}

/// `$` ile başlayan her metin referans DEĞİLDİR — meşru sabitler yayından düşmemeli.
#[test]
fn dollar_prefixed_plain_text_is_not_flagged() {
    let mut v = fixture_value();
    v["transitions"][0]["wfes_effects"]["set"]["internal_notes"] = json!("$100 ödendi");
    assert!(!has_error(&validate_value(v), "unknown_dollar_ref"));
}

/// `$env` ara-değer çözülen tek namespace'tir; biçimi kendi kuralına aittir.
#[test]
fn env_interpolation_is_not_an_unknown_ref() {
    let mut v = fixture_value();
    v["autoexec"]["kredi_skoru_getir"]["config"]["url"] = json!("$env.SCORE_API/v1/score");
    assert!(!has_error(&validate_value(v), "unknown_dollar_ref"));
}

#[test]
fn golden_fixture_has_no_unknown_dollar_refs() {
    assert!(!has_error(&validate_value(fixture_value()), "unknown_dollar_ref"));
}

/// SLA bağlamında bir ÇAĞRI DÖNÜŞÜ de yoktur: escalation'ı timer tetikler, `$call.*`
/// sessizce `null` yazardı — `$action.input.*` / `$exec.result.*` ile aynı gerekçe.
#[test]
fn sla_effects_reject_call_namespace() {
    let mut v = fixture_value();
    let node = v["nodes"].as_object().unwrap().keys().next().unwrap().clone();
    v["nodes"][&node]["escalation"] = json!([{
        "after": "PT1H",
        "wft": { "terminal": "terminal_rejected" },
        "wfes_effects": { "set": { "internal_notes": "$call.status" } }
    }]);
    assert!(has_error(&validate_value(v), "sla_effect_namespace"));
}

#[test]
fn anchor_to_actor_kinded_field_is_valid() {
    // `actor` ORGU'yu KAPSAR: içindeki orgu_id anchor'a yeter.
    let v = with_kinded_field(
        with_listable_anchor("$ctx.talep_sahibi"),
        "talep_sahibi",
        "actor",
        json!({ "user_id": { "type": "string" }, "orgu_id": { "type": "string" } }),
    );
    assert!(!has_error(
        &validate_value(v),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn anchor_to_orgu_id_child_of_kinded_field_is_valid() {
    // `$ctx.talep_sahibi.orgu_id` — ebeveynin kind'ı yeter.
    let v = with_kinded_field(
        with_listable_anchor("$ctx.talep_sahibi.orgu_id"),
        "talep_sahibi",
        "actor",
        json!({ "user_id": { "type": "string" }, "orgu_id": { "type": "string" } }),
    );
    assert!(!has_error(
        &validate_value(v),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn anchor_to_unkinded_field_is_error() {
    // `applicant` bildirilmiş bir nesne ama kind'ı yok → ORGU tuttuğu iddia edilemez.
    assert!(has_error(
        &validate_value(with_listable_anchor("$ctx.applicant")),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn anchor_to_scalar_field_is_error() {
    assert!(has_error(
        &validate_value(with_listable_anchor("$ctx.credit_score")),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn anchor_to_missing_field_is_error() {
    assert!(has_error(
        &validate_value(with_listable_anchor("$ctx.hayalet_alan")),
        "c_orgu_anchor_unknown_field"
    ));
}

#[test]
fn ctx_prefix_is_optional_in_anchor() {
    // `anchor_from_ctx` `$ctx.` önekini soyup devam ediyor — validator de aynı normalizasyonu
    // yapmalı, yoksa önek yazılmayan belge denetimden kaçar.
    assert!(has_error(
        &validate_value(with_listable_anchor("hayalet_alan")),
        "c_orgu_anchor_unknown_field"
    ));
}

#[test]
fn anchor_under_schemaless_object_is_warning_not_error() {
    // `initiated_by: {"type":"object"}` — alt yolu şema kısıtlamıyor. Meşru biçim, hata
    // değil; ama sessiz de geçmez (sessizlik kuralı atlatmanın yolu olurdu).
    let report = validate_value(with_listable_anchor("$ctx.initiated_by.orgu"));
    assert!(!has_error(&report, "c_orgu_anchor_not_orgu_kind"));
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.code == "c_orgu_anchor_kind_unverifiable"),
        "doğrulanamayan derinlik uyarı üretmeli: {:#?}",
        report.warnings
    );
}

#[test]
fn kind_behind_a_named_type_is_resolved() {
    // Motor `$ref`'i başka yerde çözmüyor ama editör `{"$ref":"#/$defs/..."}` üretebiliyor.
    // Çözülmezse MEŞRU bir belge reddedilirdi.
    let mut v = with_listable_anchor("$ctx.musteri.sube");
    v["context"]["$defs"] = json!({
        "Musteri": {
            "type": "object",
            "properties": {
                "sube": {
                    "type": "object",
                    "x-wf-kind": "orgu",
                    "properties": { "orgu_id": { "type": "string" } },
                },
            },
        },
    });
    v["context"]["properties"]["musteri"] = json!({ "format": "Musteri" });
    v["transitions"][0]["wfes_effects"]["set"]["musteri.sube"] = json!("$actor");
    let report = validate_value(v);
    assert!(
        !has_error(&report, "c_orgu_anchor_not_orgu_kind")
            && !has_error(&report, "c_orgu_anchor_unknown_field"),
        "adlandırılmış tip arkasındaki kind çözülmeli: {:#?}",
        report.errors
    );
    // Uyarının da OLMAMASI kritik: tanım çözülmeseydi yol `Opaque`'a düşer ve bu test
    // yalnız hata yokluğuna baktığı için sessizce geçerdi. Uyarısızlık, düğümün gerçekten
    // `Found(kind: orgu)` olarak çözüldüğünün kanıtı.
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.code == "c_orgu_anchor_kind_unverifiable"),
        "tanım çözümlenmiş olmalı — 'doğrulanamadı' uyarısı beklenmiyor: {:#?}",
        report.warnings
    );
}

#[test]
fn cyclic_named_types_do_not_hang() {
    let mut v = with_listable_anchor("$ctx.dongu");
    v["context"]["$defs"] = json!({
        "A": { "format": "B" },
        "B": { "format": "A" },
    });
    v["context"]["properties"]["dongu"] = json!({ "format": "A" });
    v["transitions"][0]["wfes_effects"]["set"]["dongu"] = json!("$actor");
    // Dönmemesi yeter; döngü çözülemediği için kind doğrulanamaz → uyarı.
    let report = validate_value(v);
    assert!(!has_error(&report, "c_orgu_anchor_not_orgu_kind"));
}

#[test]
fn transition_c_a_anchor_is_checked() {
    let mut v = fixture_value();
    v["transitions"][0]["c_a"] = json!({
        "c_orgu": { "from": "$ctx.applicant", "traverse": "self" },
        "c_r": ["creditAnalyst"],
    });
    assert!(has_error(
        &validate_value(v),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn x_visibility_c_orgu_anchor_is_checked() {
    // Context şemasının İÇİNDEKİ c_orgu da aynı kurala tabidir — gezinti serileştirilmiş
    // doküman üzerinde olduğu için bu yüzey kendiliğinden kapsanır.
    let mut v = fixture_value();
    v["context"]["properties"]["credit_score"]["x-visibility"] = json!({
        "c_orgu": { "from": "$ctx.applicant", "traverse": "self" },
        "c_r": ["creditAnalyst"],
    });
    assert!(has_error(
        &validate_value(v),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn node_reassign_anchor_is_checked() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["reassign"] = json!({
        "c_orgu": { "from": "$ctx.applicant", "traverse": "self" },
        "c_r": ["branchManager"],
    });
    assert!(has_error(
        &validate_value(v),
        "c_orgu_anchor_not_orgu_kind"
    ));
}

#[test]
fn selector_and_wfah_forms_are_untouched() {
    // Selector düz string; wfah formunda `from` OBJEDİR — ikisi de bu kuralın dışında.
    let mut v = fixture_value();
    v["listable"][0]["c_a"]["c_orgu"] = json!({
        "from": { "wfah": "analyst_approve", "field": "actor.orgu" },
        "traverse": "self.parent",
    });
    let report = validate_value(v);
    assert!(!has_error(&report, "c_orgu_anchor_not_orgu_kind"));
    assert!(!has_error(&report, "c_orgu_anchor_unknown_field"));
    assert!(!report
        .warnings
        .iter()
        .any(|w| w.code == "c_orgu_anchor_kind_unverifiable"));
}

// ---- c_u: sabit kimlik ↔ context referansı ----
//
// `c_u` öğesi ya `Literal` (kullanıcı adı/UUID) ya `Ref {from: "$ctx..."}`. İki kural
// birleşimin iki ucundaki SESSİZ başarısızlıkları kapatıyor: `$` ile başlayan bir sabit
// kimlik motorda "böyle bir kullanıcı adı" sanılıp hiç eşleşmez; `actor` kind'lı olmayan
// bir yola bakan referans runtime'da çözülemez ve havuz sessizce daralır.

/// `c_u`'yu verilen öğelerle kuran bir listable kuralı (node c_a'sı DEĞİL — orada §2b
/// tekilliği `duplicate_c_a` tetiklenebilir ve ölçüm bulanıklaşır).
fn with_listable_cu(items: Value) -> Value {
    let mut v = fixture_value();
    v["listable"][0]["c_a"]["c_u"] = items;
    v
}

/// `actor` kind'lı bir alan + onu yazan effect (WOR-70: her alanın bir yazarı olmalı).
fn with_actor_field(mut v: Value, name: &str) -> Value {
    v["context"]["properties"][name] = json!({
        "type": "object",
        "x-wf-kind": "actor",
        "properties": {
            "user_id": { "type": "string" },
            "orgu_id": { "type": "string" },
            "role": { "type": "string" },
        },
    });
    v["transitions"][0]["wfes_effects"]["set"][name] = json!("$actor");
    v
}

#[test]
fn plain_string_c_u_still_valid() {
    // Geriye uyumluluk: düz string listesi aynen çalışmalı (Literal'a deserialize olur).
    let report = validate_value(with_listable_cu(json!(["ahmet.yilmaz"])));
    assert!(!has_error(&report, "c_u_literal_dollar_prefix"));
    assert!(!has_error(&report, "c_u_ref_not_actor_kind"));
}

#[test]
fn c_u_ref_to_actor_field_is_valid() {
    let v = with_actor_field(
        with_listable_cu(json!([{ "from": "$ctx.talep_sahibi.user_id" }])),
        "talep_sahibi",
    );
    let report = validate_value(v);
    assert!(
        !has_error(&report, "c_u_ref_not_actor_kind") && !has_error(&report, "c_u_ref_unknown_field"),
        "actor kind'lı alanın user_id'sine bakan referans geçerli olmalı: {:#?}",
        report.errors
    );
}

#[test]
fn c_u_ref_to_actor_object_itself_is_valid() {
    // Son ek yazmadan: `resolve_cu_ident` nesne içinde user_id arar.
    let v = with_actor_field(
        with_listable_cu(json!([{ "from": "$ctx.talep_sahibi" }])),
        "talep_sahibi",
    );
    assert!(!has_error(&validate_value(v), "c_u_ref_not_actor_kind"));
}

#[test]
fn c_u_ref_to_orgu_kinded_field_is_error() {
    // `orgu` kind'ı YETMEZ — içinde kişi yoktur. Bu, `c_orgu`'nun tersi yöndeki kısıt:
    // orada `actor` kabul edilir (orgu_id taşır), burada `orgu` KABUL EDİLMEZ.
    let mut v = with_listable_cu(json!([{ "from": "$ctx.musteri_sube" }]));
    v["context"]["properties"]["musteri_sube"] = json!({
        "type": "object",
        "x-wf-kind": "orgu",
        "properties": { "orgu_id": { "type": "string" } },
    });
    v["transitions"][0]["wfes_effects"]["set"]["musteri_sube"] = json!("$actor");
    assert!(has_error(&validate_value(v), "c_u_ref_not_actor_kind"));
}

#[test]
fn c_u_ref_to_plain_field_is_error() {
    assert!(has_error(
        &validate_value(with_listable_cu(json!([{ "from": "$ctx.applicant" }]))),
        "c_u_ref_not_actor_kind"
    ));
}

#[test]
fn c_u_ref_to_missing_field_is_error() {
    assert!(has_error(
        &validate_value(with_listable_cu(json!([{ "from": "$ctx.hayalet" }]))),
        "c_u_ref_unknown_field"
    ));
}

#[test]
fn c_u_literal_starting_with_dollar_is_error() {
    // Asıl yakalanan hata: tasarımcı `Ref` yazmayı unutup düz string bırakmış.
    // Motor bunu kullanıcı adı sanar ve kural HİÇ eşleşmez — sessiz, izsiz.
    assert!(has_error(
        &validate_value(with_listable_cu(json!(["$ctx.talep_sahibi.user_id"]))),
        "c_u_literal_dollar_prefix"
    ));
}

#[test]
fn c_u_literal_typo_prefix_is_error() {
    // `$actor.user_id` de aynı tuzağa düşer — `$` ile başlayan HER sabit kimlik yakalanır.
    assert!(has_error(
        &validate_value(with_listable_cu(json!(["$actor.user_id"]))),
        "c_u_literal_dollar_prefix"
    ));
}

#[test]
fn c_u_ref_and_literal_can_mix() {
    // Birleşimin amacı: aynı dizide statik ve dinamik öğeler yan yana durabilsin.
    let v = with_actor_field(
        with_listable_cu(json!(["ahmet.yilmaz", { "from": "$ctx.talep_sahibi.user_id" }])),
        "talep_sahibi",
    );
    let report = validate_value(v);
    assert!(report
        .errors
        .iter()
        .all(|e| !e.code.starts_with("c_u_")), "karışık liste geçerli olmalı: {:#?}", report.errors);
}

#[test]
fn x_visibility_c_u_is_checked_too() {
    // x-visibility'nin c_u'su C_A'nınkiyle AYNI şekildedir (terminology.md) — gezinti
    // serileştirilmiş doküman üzerinde olduğu için bu yüzey kendiliğinden kapsanır.
    let mut v = fixture_value();
    v["context"]["properties"]["credit_score"]["x-visibility"] =
        json!({ "c_u": [{ "from": "$ctx.applicant" }] });
    assert!(has_error(&validate_value(v), "c_u_ref_not_actor_kind"));
}

// NOT (2026-08-14): `node_key_unchanged_by_cu_item_union` SİLİNDİ. Kalkmış bir kuralı
// ölçüyordu (`check_slugs` → `slug` hata kodu, 2026-08-12'de kaldırıldı), dolayısıyla
// assert'i boşa geçiyordu; koştuğu şey (değiştirilmemiş golden fixture'ın temiz
// doğrulanması) `golden_fixture_is_valid` ile birebir aynıydı. Node anahtarı artık
// `c_u` biçiminden ETKİLENEMEZ: anahtarı tasarımcı yazar, motor türetmez.

// ─── Sayısal agregat (sum/avg/min/max/median/mode) ────────────────
//
// Regresyon: `type_of` fonksiyon sonucunu bilerek `Unknown` sayar, dolayısıyla
// `avg(map($wfah, #.action)) > 0` HİÇBİR kurala takılmıyordu — editörün toplama satırı
// bunu üretebiliyor, yayın kapısı "temiz" diyor, koşul çalışma anında
// "Expected a number array" ile patlıyordu.

#[test]
fn numeric_agg_over_text_field_is_error() {
    assert!(
        errors_for_when("avg(map($wfah, #.action)) > 0").contains(&"zen_agg_not_numeric".to_string()),
        "metin alanında ortalama reddedilmeli"
    );
    for fnname in ["sum", "avg", "min", "max", "median", "mode"] {
        assert!(
            errors_for_when(&format!("{fnname}(map($wfah, #.actor.role)) > 0"))
                .contains(&"zen_agg_not_numeric".to_string()),
            "{fnname} metin dizisinde reddedilmeli"
        );
    }
}

#[test]
fn numeric_agg_over_object_field_is_error() {
    // `#.actor` bir nesnedir — dizi elemanı olarak da sayı değildir.
    assert!(errors_for_when("sum(map($wfah, #.actor)) > 0")
        .contains(&"zen_agg_not_numeric".to_string()));
}

#[test]
fn numeric_agg_over_number_field_is_clean() {
    let report = validate_value(fixture_with_when("sum(map($wfah, #.seq)) > 0"));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn numeric_agg_over_filtered_history_is_clean() {
    let report = validate_value(fixture_with_when(
        r#"sum(map(filter($wfah, #.action == "analyst_approve"), #.seq)) > 0"#,
    ));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn unguarded_avg_over_whole_history_warns_but_publishes() {
    // Boş geçmişte `avg([])` patlar — ama koşulun konulduğu yerde geçmişin dolu olduğu
    // garanti olabilir, o yüzden UYARI (yayını engellemez).
    let report = validate_value(fixture_with_when("avg(map($wfah, #.seq)) > 0"));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
    let codes: Vec<String> = report.warnings.into_iter().map(|w| w.code).collect();
    assert!(
        codes.contains(&"zen_agg_empty_history".to_string()),
        "süzgeçsiz avg uyarı vermeli, uyarılar: {codes:#?}"
    );
}

#[test]
fn sum_over_whole_history_does_not_warn() {
    // `sum([])` sıfırdır — boş geçmiş riski YOK.
    let report = validate_value(fixture_with_when("sum(map($wfah, #.seq)) > 0"));
    let codes: Vec<String> = report.warnings.into_iter().map(|w| w.code).collect();
    assert!(!codes.contains(&"zen_agg_empty_history".to_string()), "uyarılar: {codes:#?}");
}

// ---- Çapasız C_A (c_orgu yok) — biçim kuralları -------------------------------------

/// Golden fixture'ın bir node'unu çapasız (yalnız c_u) biçime çevirir. Node key de
/// `any__u_ayse` olarak yeniden yazılır — bu ARTIK ZORUNLU DEĞİL (kimliği tasarımcı
/// verir, `slug`/`node_key_slug` kuralları 2026-08-12'de kalktı); anahtar yalnız
/// okunurluk için c_a ile aynı hikâyeyi anlatsın diye değiştirilmeye devam ediyor.
fn fixture_with_anchorless_node(c_a: Value) -> Value {
    let mut v = fixture_value();
    let nodes = v["nodes"].as_object_mut().unwrap();
    let old_key = nodes.keys().next().unwrap().clone();
    let mut node = nodes.remove(&old_key).unwrap();
    node["c_a"] = c_a;
    let new_key = "any__u_ayse".to_string();
    nodes.insert(new_key.clone(), node);
    // Node key dokümanın her yerinden referanslanır (start.from, transitions.from,
    // wft.node, escalation[].wft, listable[]...) — tek tek gezmek yerine TÜM string'ler
    // yeniden yazılır. Test fixture'ında bu metin başka bir anlamda kullanılmıyor.
    rename_strings(&mut v, &old_key, &new_key);
    v
}

fn rename_strings(v: &mut Value, from: &str, to: &str) {
    match v {
        Value::String(s) => {
            if s == from {
                *s = to.to_string();
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rename_strings(x, from, to)),
        Value::Object(m) => m.iter_mut().for_each(|(_, x)| rename_strings(x, from, to)),
        _ => {}
    }
}

#[test]
fn anchorless_c_u_only_node_is_valid() {
    let report = validate_value(fixture_with_anchorless_node(json!({ "c_u": ["ayse"] })));
    assert!(report.errors.is_empty(), "hatalar: {:#?}", report.errors);
}

#[test]
fn anchorless_with_c_r_is_error() {
    let report = validate_value(fixture_with_anchorless_node(
        json!({ "c_u": ["ayse"], "c_r": ["mudur"] }),
    ));
    assert!(has_error(&report, "c_a_anchorless_role"), "{:#?}", report.errors);
}

#[test]
fn anchorless_without_c_u_is_error() {
    let report = validate_value(fixture_with_anchorless_node(json!({})));
    assert!(
        has_error(&report, "c_a_anchorless_needs_user"),
        "{:#?}",
        report.errors
    );
}

/// `reassign` de C_A şeklindedir — aynı kısıt orada da geçerli olmalı, yoksa çapasız
/// rol kanalı devir yetkisi üzerinden geri sızardı.
#[test]
fn anchorless_reassign_with_c_r_is_error() {
    let mut v = fixture_value();
    let nodes = v["nodes"].as_object_mut().unwrap();
    let key = nodes.keys().next().unwrap().clone();
    nodes[&key]["reassign"] = json!({ "c_r": ["mudur"] });
    let report = validate_value(v);
    assert!(has_error(&report, "c_a_anchorless_role"), "{:#?}", report.errors);
}

// ================================================================ T‑A5: wf_admin
// Akış-içi yetkili kuralları `listable` ile AYNI denetimlerden geçer.

/// `wf_admin[].when` ifade denetimine girer — bozuk guard yayına çıkmaz.
#[test]
fn wf_admin_when_expression_is_validated() {
    let mut v = fixture_value();
    v["wf_admin"] = json!([{
        "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
        // count'un iki argümanlı olması gerekir (WOR-84) — bu form parse hatası verir.
        "when": "count(filter($wfah, #.action == \"Onayla\")) > 0"
    }]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.path.starts_with("wf_admin[0].when")),
        "wf_admin[].when denetlenmeli, hatalar: {:#?}",
        report.errors
    );
}

/// Geçerli `wf_admin` kuralı temiz geçer.
#[test]
fn valid_wf_admin_rule_passes() {
    let mut v = fixture_value();
    v["wf_admin"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.credit_info.amount_requested > 100000" }
    ]);
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "geçerli wf_admin hata üretmemeli: {:#?}",
        report.errors
    );
}

/// Çapasız `wf_admin` kuralında `c_r` YASAK — kural `c_a` şekil denetimine dahildir
/// (en geniş kapı: "tenant'taki tüm müdürler").
#[test]
fn anchorless_wf_admin_rule_cannot_carry_role() {
    let mut v = fixture_value();
    v["wf_admin"] = json!([{ "c_a": { "c_r": ["branchManager"], "c_u": ["ahmet"] } }]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "c_a_anchorless_role"),
        "çapasız wf_admin kuralı rol taşımamalı: {:#?}",
        report.errors
    );
}

/// Ne çapa ne kişi taşıyan kural HİÇ KİMSEYLE eşleşmez → hata.
#[test]
fn empty_wf_admin_rule_is_rejected() {
    let mut v = fixture_value();
    v["wf_admin"] = json!([{ "c_a": {} }]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "c_a_anchorless_needs_user"),
        "boş kural reddedilmeli: {:#?}",
        report.errors
    );
}

// ================================ görünürlük grant guard'ları (2026-08-13)

/// `listable[].when` içinde `$actor` YASAK.
///
/// Gerekçe: bu guard'lar görünürlük projeksiyonuna (`wf.wfe.view_c_a`) commit
/// anında, soruyu soracak kişi HENÜZ BİLİNMEZKEN yazılır. `$actor` guard'ı
/// viewer'a bağlar → aynı WFE iki kişiye iki farklı cevap verir ve tek bir
/// kolona yazılamaz. Kapı yayında keser, yoksa hata üretim zamanında ve sessizce
/// (grant hiç yazılmayarak) ortaya çıkardı.
#[test]
fn listable_when_rejects_actor_reference() {
    let mut v = fixture_value();
    v["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
          "when": "$actor.role == \"branchManager\"" }
    ]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "grant_when_actor_ref"),
        "grant_when_actor_ref beklenmişti, hatalar: {:#?}",
        report.errors
    );
}

/// Aynı kısıt `wf_admin[].when` için de geçerli — iki kural aynı şekli
/// (`CaGrantRule`) taşır ve aynı projeksiyona yazılır.
#[test]
fn wf_admin_when_rejects_actor_reference() {
    let mut v = fixture_value();
    v["wf_admin"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
          "when": "$actor.orgu_id != null" }
    ]);
    let report = validate_value(v);
    assert!(has_error(&report, "grant_when_actor_ref"));
}

/// `$actor` İÇERMEYEN guard serbest: kısıt yalnız viewer bağımlılığınadır,
/// guard'ın kendisine değil. (Aksi halde `when` özelliği işlevsiz kalırdı.)
#[test]
fn listable_when_allows_ctx_reference() {
    let mut v = fixture_value();
    v["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
          "when": "$ctx.credit_info.amount_requested >= 100000" }
    ]);
    let report = validate_value(v);
    assert!(!has_error(&report, "grant_when_actor_ref"));
}

/// Node AKSİYONLARININ `when`i bu kısıttan ETKİLENMEZ: orada `$actor` geçişi
/// yapan kişidir ve karar anında bilinir. Kapının kapsamını sabitler.
#[test]
fn transition_when_still_allows_actor_reference() {
    let mut v = fixture_value();
    let t = v["transitions"][0].clone();
    let mut t2 = t.clone();
    t2["when"] = json!("$actor.role == \"branchClerk\"");
    v["transitions"][0] = t2;
    let report = validate_value(v);
    assert!(!has_error(&report, "grant_when_actor_ref"));
}

// ============================================ node-seviyesi `listable[]` (2026-08-13)
// node-listable-design.md: `nodes.<key>.listable[]` kök `listable`/`wf_admin` ile AYNI
// şekli (`CaGrantRule`) ve AYNI denetimlerden geçer — tek fark ömür (duruma bağlı).

/// Geçerli bir node `listable[]` kuralı temiz geçer.
#[test]
fn valid_node_listable_rule_passes() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.credit_info.amount_requested > 100000" }
    ]);
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "geçerli node listable hata üretmemeli: {:#?}",
        report.errors
    );
}

/// `nodes.<key>.listable[].when` ifade denetimine girer — kök `listable[].when`
/// ile AYNI kapı (expr_types tip denetimi dahil, runtime-semantics.md §6a-1).
#[test]
fn node_listable_when_expression_is_validated() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([{
        "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
        // count'un iki argümanlı olması gerekir (WOR-84) — bu form parse hatası verir.
        "when": "count(filter($wfah, #.action == \"Onayla\")) > 0"
    }]);
    let report = validate_value(v);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.path.starts_with("nodes[self__creditAnalyst].listable[0].when")),
        "nodes.<key>.listable[].when denetlenmeli, hatalar: {:#?}",
        report.errors
    );
}

/// `nodes.<key>.listable[].when` içinde `$actor` YASAK — kök `listable`/`wf_admin`
/// guard'ıyla AYNI gerekçe: bu kural görünürlük projeksiyonuna viewer henüz
/// bilinmezken yazılır.
#[test]
fn node_listable_when_rejects_actor_reference() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
          "when": "$actor.role == \"branchManager\"" }
    ]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "grant_when_actor_ref"),
        "grant_when_actor_ref beklenmişti, hatalar: {:#?}",
        report.errors
    );
}

/// `$actor` İÇERMEYEN guard serbest — node listable'da da `when` işlevsiz kalmamalı.
#[test]
fn node_listable_when_allows_ctx_reference() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] },
          "when": "$ctx.credit_info.amount_requested >= 100000" }
    ]);
    let report = validate_value(v);
    assert!(!has_error(&report, "grant_when_actor_ref"), "{:#?}", report.errors);
}

/// Çapasız (`c_orgu` yok) node `listable[]` kuralında `c_r` YASAK — `check_c_a_shape`
/// dokümanı doc-geniş (`serde_json::to_value(wfd)`) tarar, node listable otomatik dahildir.
#[test]
fn node_listable_anchorless_rule_cannot_carry_role() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] =
        json!([{ "c_a": { "c_r": ["branchManager"], "c_u": ["ahmet"] } }]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "c_a_anchorless_role"),
        "çapasız node listable kuralı rol taşımamalı: {:#?}",
        report.errors
    );
}

/// Ne çapa ne kişi taşıyan node `listable[]` kuralı HİÇ KİMSEYLE eşleşmez → hata.
#[test]
fn node_listable_empty_rule_is_rejected() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([{ "c_a": {} }]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "c_a_anchorless_needs_user"),
        "boş node listable kuralı reddedilmeli: {:#?}",
        report.errors
    );
}

/// `c_orgu` anchor'ı context şemasında olmayan bir alanı işaret ediyorsa node
/// `listable[]` de kök `listable` ile AYNI kapıdan geçer (`check_c_orgu_anchor_kinds`
/// — doc-geniş tarama, `x-wf-kind` bilgisi context şemasından okunur).
#[test]
fn node_listable_bad_c_orgu_anchor_is_caught() {
    let mut v = fixture_value();
    v["nodes"]["self__creditAnalyst"]["listable"] = json!([{
        "c_a": { "c_orgu": { "from": "$ctx.no_such_field", "traverse": "self" },
                 "c_u": ["ahmet"] }
    }]);
    let report = validate_value(v);
    assert!(
        has_error(&report, "c_orgu_anchor_unknown_field"),
        "node listable c_orgu anchor'ı context şemasıyla doğrulanmalı: {:#?}",
        report.errors
    );
}

// ---- Adlandırılmış tip: `format` → `$defs` (2026-08-19) ----
//
// `format` bu belgede standart JSON Schema formatı DEĞİL, `$defs`'teki bir tipin adıdır.
// Kural motorda çünkü çalışma anı denetimi (`v22::ctx_types`) de aynı çözümü kullanıyor:
// editör/portal yalnız aynı cevabı önden verir.

#[test]
fn named_type_via_format_is_valid_and_typed() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Para": { "type": "number", "minimum": 0 } });
    // `credit_info.amount_requested` golden'da `number`; aynı tipi ADLA veriyoruz.
    v["context"]["properties"]["credit_info"]["properties"]["amount_requested"] =
        json!({ "format": "Para" });
    let report = validate_value(v);
    assert!(
        report.errors.is_empty(),
        "adlandırılmış tip geçerli olmalı: {:#?}",
        report.errors
    );
}

#[test]
fn unknown_format_name_is_error() {
    let mut v = fixture_value();
    v["context"]["properties"]["credit_grade"] = json!({ "format": "BoyleBirTipYok" });
    let report = validate_value(v);
    assert!(has_error(&report, "context_format_unknown"), "{:#?}", report.errors);
}

/// `format` bir tanıma işaret ettiği için tip kuralı YANINA yazılamaz — tip tanımın
/// içindedir. Aksi halde "hangisi kazanıyor" belirsiz kalırdı.
#[test]
fn format_next_to_a_type_keyword_is_error() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Metin": { "type": "string" } });
    v["context"]["properties"]["credit_grade"] = json!({ "format": "Metin", "type": "string" });
    let report = validate_value(v);
    assert!(has_error(&report, "context_format_with_type"), "{:#?}", report.errors);
}

/// Anlatım/görünürlük anahtarları kullanım yerinde EZİLEBİLİR (eski `$ref` davranışı).
#[test]
fn description_next_to_format_is_allowed() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Metin": { "type": "string" } });
    v["context"]["properties"]["credit_grade"] =
        json!({ "format": "Metin", "description": "bu alana özel açıklama" });
    let report = validate_value(v);
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
}

#[test]
fn cyclic_type_definitions_are_error() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "A": { "format": "B" }, "B": { "format": "A" } });
    v["context"]["properties"]["credit_grade"] = json!({ "format": "A" });
    let report = validate_value(v);
    assert!(has_error(&report, "context_format_cycle"), "{:#?}", report.errors);
}

#[test]
fn invalid_definition_name_is_error() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "1Para": { "type": "number" } });
    let report = validate_value(v);
    assert!(has_error(&report, "context_defs_name"), "{:#?}", report.errors);
}

/// `$ref` YAZILAMAZ (kapı yalnız yazma yollarında) — okuma tarafı onu hâlâ çözer,
/// bkz. `v22::ctx_types::tests::legacy_ref_is_still_resolved`.
#[test]
fn writing_ref_is_error() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Metin": { "type": "string" } });
    v["context"]["properties"]["credit_grade"] = json!({ "$ref": "#/$defs/Metin" });
    let report = validate_value(v);
    assert!(has_error(&report, "context_ref_removed"), "{:#?}", report.errors);
}

/// Adlandırılmış tip artık effect TİP denetimine de girer: `$defs` arkasındaki alan
/// eskiden `Opaque`ti ve `effect_type_mismatch` onu hiç görmüyordu.
#[test]
fn effect_type_mismatch_sees_through_a_named_type() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Metin": { "type": "string" } });
    v["context"]["properties"]["credit_grade"] = json!({ "format": "Metin" });
    // `$actor` bir NESNEDİR; metin bir alana yazılamaz.
    v["transitions"][0]["wfes_effects"]["set"]["credit_grade"] = json!("$actor");
    let report = validate_value(v);
    assert!(has_error(&report, "effect_type_mismatch"), "{:#?}", report.errors);
}

/// Girdi yolu denetimi de tanımın arkasını görür: adlandırılmış tipli bir alan
/// `input.required`da meşru bir yoldur (eskiden `Opaque` olduğu için sessizce geçiyordu,
/// şimdi gerçekten ÇÖZÜLÜYOR).
#[test]
fn declared_input_path_through_a_named_type_resolves() {
    let mut v = fixture_value();
    v["context"]["$defs"] = json!({ "Musteri": { "type": "object", "properties": {
        "name": { "type": "string" }, "tckid": { "type": "string" }, "income": { "type": "number" }
    }}});
    v["context"]["properties"]["applicant"] = json!({ "format": "Musteri" });
    let report = validate_value(v);
    assert!(
        !has_error(&report, "input_path"),
        "`applicant` tanım arkasında da çözülmeli: {:#?}",
        report.errors
    );
}
