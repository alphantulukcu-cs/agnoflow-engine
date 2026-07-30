//! WFC (İş Akışı Çağrısı) validator testleri.
//!
//! Üç mod: `wait` / `detached` (node yerleşimi, `nodes.<k>.call`) ve `terminal`
//! (ardıl akış, `terminals[].call`). Plan: docs/plans/workflow-call.md.
//!
//! Cross-WFD kurallar `WfdProvider` gerektirir; `validate()` (resolver'sız) yalnız yerel
//! kuralları koşar. Buradaki `Catalog` sahte bir resolver'dır.

use serde_json::{json, Value};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::validator::{validate, validate_with, ValidationReport, WfdProvider};

const CALLER: &str = include_str!("fixtures/akis-cagrisi.json");
const SKOR: &str = include_str!("fixtures/kredi-skor.json");
const KULLANDIRIM: &str = include_str!("fixtures/kredi-kullandirim.json");

/// Sahte WFD deposu. `version: None` = "en son yayınlanmış" — testte tek sürüm var.
struct Catalog(Vec<Wfd>);

impl Catalog {
    fn new() -> Self {
        Catalog(
            [SKOR, KULLANDIRIM]
                .iter()
                .map(|s| Wfd::from_json(s).expect("çağrılan fixture parse edilebilir"))
                .collect(),
        )
    }

    /// Ek bir WFD ile (döngü senaryoları için) genişletilmiş katalog.
    fn with(mut self, extra: Value) -> Self {
        self.0
            .push(Wfd::from_value(extra).expect("ek WFD parse edilebilir"));
        self
    }
}

impl WfdProvider for Catalog {
    fn resolve(&self, wfd_id: &str, version: Option<&str>) -> Option<Wfd> {
        self.0
            .iter()
            .find(|w| w.id == wfd_id && version.map_or(true, |v| w.version == v))
            .cloned()
    }
}

fn caller_value() -> Value {
    serde_json::from_str(CALLER).unwrap()
}

fn parse(v: Value) -> Wfd {
    Wfd::from_value(v).expect("mutasyon parse edilebilir kalmalı")
}

/// Yalnız yerel kurallar (resolver yok).
fn local(v: Value) -> ValidationReport {
    validate(&parse(v))
}

/// Yerel + cross-WFD kurallar.
fn full(v: Value) -> ValidationReport {
    validate_with(&parse(v), Some(&Catalog::new()))
}

fn full_with(v: Value, catalog: Catalog) -> ValidationReport {
    validate_with(&parse(v), Some(&catalog))
}

fn has_error(report: &ValidationReport, code: &str) -> bool {
    report.errors.iter().any(|e| e.code == code)
}

fn has_warning(report: &ValidationReport, code: &str) -> bool {
    report.warnings.iter().any(|w| w.code == code)
}

/// Fixture'ın belirli bir alanını değiştirir (dotted olmayan, elle gezinen yol).
fn mutate(f: impl FnOnce(&mut Value)) -> Value {
    let mut v = caller_value();
    f(&mut v);
    v
}

// ---- temel: fixture'lar temiz ----

#[test]
fn caller_fixture_is_valid_with_and_without_resolver() {
    for report in [local(caller_value()), full(caller_value())] {
        assert!(
            report.errors.is_empty(),
            "çağıran fixture temiz geçmeli, hatalar: {:#?}",
            report.errors
        );
        assert!(
            report.warnings.is_empty(),
            "çağıran fixture uyarısız geçmeli, uyarılar: {:#?}",
            report.warnings
        );
    }
}

#[test]
fn callee_fixtures_are_valid() {
    for src in [SKOR, KULLANDIRIM] {
        let wfd = Wfd::from_json(src).unwrap();
        let report = validate(&wfd);
        assert!(
            report.errors.is_empty(),
            "çağrılan '{}' temiz geçmeli: {:#?}",
            wfd.id,
            report.errors
        );
    }
}

/// Golden fixture WFC bilmiyor — `calls` yoksa hiçbir WFC kuralı tetiklenmemeli
/// (yeni alanların hepsi opsiyonel, geriye dönük uyumluluk).
#[test]
fn wfd_without_calls_triggers_no_call_rules() {
    let golden: Value =
        serde_json::from_str(include_str!("fixtures/kredi-basvuru.golden.json")).unwrap();
    let report = full(golden);
    assert!(
        !report.errors.iter().any(|e| e.code.starts_with("call_")),
        "WFC'siz WFD'de WFC hatası olmamalı: {:#?}",
        report.errors
    );
}

// ---- mod ↔ yerleşim eşlemesi ----

#[test]
fn terminal_mode_on_a_node_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]["mode"] = json!("terminal");
    });
    assert!(has_error(&local(v), "call_mode_placement"));
}

#[test]
fn wait_mode_on_a_terminal_is_rejected() {
    let v = mutate(|v| {
        v["terminals"][0]["call"]["mode"] = json!("wait");
    });
    assert!(has_error(&local(v), "call_mode_placement"));
}

/// `mode` default'u `wait`'tir — terminal'de mod yazılmazsa yerleşim kuralı yakalar.
/// Böylece "terminal'de mode zorunludur" ayrı bir kural gerektirmez.
#[test]
fn missing_mode_on_a_terminal_is_caught_by_placement() {
    let v = mutate(|v| {
        v["terminals"][0]["call"]
            .as_object_mut()
            .unwrap()
            .remove("mode");
    });
    assert!(has_error(&local(v), "call_mode_placement"));
}

// ---- katalog referansları ----

#[test]
fn unknown_catalog_key_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]["use"] = json!("yok_boyle_bir_cagri");
    });
    assert!(has_error(&local(v), "call_unknown_use"));
}

#[test]
fn unused_catalog_entry_warns() {
    let v = mutate(|v| {
        v["calls"]["hic_kullanilmayan"] = json!({
            "wfd_id": "kredi-skor",
            "input": { "musteri_no": "$ctx.basvuru.musteri_no", "talep_tutari": "$ctx.basvuru.tutar" }
        });
    });
    assert!(has_warning(&local(v), "call_unused_catalog_entry"));
}

// ---- WFC-IN: girdi sözleşmesi ----

/// Kullanıcının çekirdek talebi: çağrılanın girdileri çağıranın context'inde
/// BULUNMAK ZORUNDA. Bu kural resolver GEREKTİRMEZ — yerel olarak yakalanır.
#[test]
fn input_source_must_exist_in_caller_context() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["musteri_no"] = json!("$ctx.hic_olmayan_alan");
    });
    let report = local(v);
    assert!(has_error(&report, "call_input_source_undeclared"));
    // Generic `ctx_ref` ile ÇİFT raporlanmamalı (calls alt ağacı generic gezginden muaf).
    assert!(!has_error(&report, "ctx_ref"));
}

#[test]
fn action_input_namespace_is_banned_in_call_input() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["musteri_no"] = json!("$action.input.basvuru");
    });
    assert!(has_error(&local(v), "call_input_namespace"));
}

#[test]
fn call_result_namespace_is_banned_in_call_input() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["musteri_no"] = json!("$call.result.skor");
    });
    assert!(has_error(&local(v), "call_input_namespace"));
}

#[test]
fn literal_and_actor_inputs_are_allowed() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["musteri_no"] = json!("sabit-deger");
        v["calls"]["kredi_kullandirim"]["input"]["musteri_no"] = json!("$actor");
    });
    let report = local(v);
    assert!(!has_error(&report, "call_input_namespace"));
    assert!(!has_error(&report, "call_input_source_undeclared"));
}

#[test]
fn missing_required_callee_input_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]
            .as_object_mut()
            .unwrap()
            .remove("talep_tutari");
    });
    // Yerel kurallar bunu göremez — çağrılanın sözleşmesi gerekir.
    assert!(!has_error(&local(caller_value()), "call_input_missing"));
    assert!(has_error(&full(v), "call_input_missing"));
}

#[test]
fn unknown_callee_input_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["boyle_bir_girdi_yok"] = json!("x");
    });
    assert!(has_error(&full(v), "call_input_unknown"));
}

/// Kaynak alan `string`, çağrılanın hedef alanı `number` → tip uyuşmazlığı.
#[test]
fn input_type_mismatch_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["talep_tutari"] =
            json!("$ctx.basvuru.musteri_no");
    });
    assert!(has_error(&full(v), "call_input_type_mismatch"));
}

/// `integer` bir `number` yerine geçer — gereksiz katılık üretmemeli.
#[test]
fn integer_source_satisfies_number_target() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["talep_tutari"] = json!("$ctx.skor");
    });
    assert!(!has_error(&full(v), "call_input_type_mismatch"));
}

#[test]
fn literal_type_is_checked_too() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["input"]["talep_tutari"] = json!("bu-bir-metin");
    });
    assert!(has_error(&full(v), "call_input_type_mismatch"));
}

/// Çağrılanın ≥2 start kuralı varsa hangisiyle başlatılacağı belirtilmelidir.
#[test]
fn ambiguous_callee_start_requires_explicit_choice() {
    let mut extra: Value = serde_json::from_str(SKOR).unwrap();
    extra["id"] = json!("cok-startli");
    // ikinci start kuralı: aynı node, aynı aksiyon — yalnız id farklı
    let second = extra["start"][0].clone();
    extra["start"] = json!([extra["start"][0].clone(), second]);
    extra["start"][1]["id"] = json!("start__ikinci");

    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["wfd_id"] = json!("cok-startli");
        v["calls"]["kredi_skor_sorgusu"]["version"] = json!("1.0.0");
        v["calls"]["kredi_skor_sorgusu"]
            .as_object_mut()
            .unwrap()
            .remove("start");
    });
    assert!(has_error(
        &full_with(v, Catalog::new().with(extra)),
        "call_start_ambiguous"
    ));
}

// ---- WFC-OUT: `$call.*` ----

/// `$call.result.<k>` çağrılanın hiçbir terminal'inin yanıtında yoksa daima null olur.
#[test]
fn unknown_call_result_key_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]["wfes_effects"]["set"]["skor"] =
            json!("$call.result.boyle_bir_alan_yok");
    });
    assert!(has_error(&full(v), "call_result_unknown"));
}

/// `detached` çağrının sonucunu hiç beklemez — `$call.result.*` orada anlamsızdır.
#[test]
fn call_result_in_detached_mode_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]["mode"] = json!("detached");
    });
    assert!(has_error(&local(v), "call_result_in_detached"));
}

#[test]
fn detached_mode_rejects_timeout() {
    let v = mutate(|v| {
        let call = &mut v["nodes"]["self__creditAnalyst"]["call"];
        call["mode"] = json!("detached");
        call["wfes_effects"]["set"] = json!({ "skor_durumu": "$call.status" });
        call["wfes_effects"]["set"]["skor"] = json!(0);
    });
    assert!(has_error(&local(v), "call_node_forbidden_field"));
}

// ---- WFC node kısıtları ----

#[test]
fn call_node_requires_wft() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]
            .as_object_mut()
            .unwrap()
            .remove("wft");
    });
    assert!(has_error(&local(v), "call_wft_required"));
}

/// WFC node'u bir bekleme HAVUZU değildir — insan aksiyonu alınamaz.
#[test]
fn transition_from_a_call_node_is_rejected() {
    let v = mutate(|v| {
        let t = json!({
            "id": "t_elle_gec",
            "from": "self__creditAnalyst",
            "action": "manager_decide",
            "wfes_effects": { "set": { "kullandirim_tutari": "$action.input.kullandirim_tutari" } },
            "wft": { "terminal": "terminal_rejected" }
        });
        v["transitions"].as_array_mut().unwrap().push(t);
    });
    assert!(has_error(&local(v), "call_node_has_action"));
}

#[test]
fn escalation_on_a_call_node_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["escalation"] = json!([
            { "after": "P1D", "wft": { "node": "self__branchManager" } }
        ]);
    });
    assert!(has_error(&local(v), "call_node_forbidden_field"));
}

#[test]
fn claim_timeout_on_a_call_node_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["claim_timeout"] = json!({ "after": "PT4H" });
    });
    assert!(has_error(&local(v), "call_node_forbidden_field"));
}

#[test]
fn start_node_cannot_be_a_call_node() {
    let v = mutate(|v| {
        // start node'unu WFC node'u yap
        v["nodes"]["type_branch__branchClerk"]["call"] = json!({
            "use": "kredi_skor_sorgusu",
            "mode": "wait",
            "wft": { "node": "self__branchManager" }
        });
    });
    assert!(has_error(&local(v), "call_node_is_start"));
}

/// WFC-RETURN'ü system tetikler — aksiyon girdisi yoktur.
#[test]
fn action_input_in_return_effects_is_rejected() {
    let v = mutate(|v| {
        v["nodes"]["self__creditAnalyst"]["call"]["wfes_effects"]["set"]["skor"] =
            json!("$action.input.basvuru");
    });
    assert!(has_error(&local(v), "call_effect_namespace"));
}

/// WFC node'unun çıkışı `call.wft`'dir — transition aranmadığı için `no_exit`
/// yanlış alarm vermemeli; hedefleri de `unreachable` görünmemeli.
#[test]
fn call_node_exit_participates_in_graph_checks() {
    let report = local(caller_value());
    assert!(!has_error(&report, "no_exit"));
    assert!(!has_error(&report, "unreachable"));
}

/// Alt akıştan dönen alanı yalnız WFC-RETURN yazıyor — `context_field_never_written`
/// bunu yazar saymalı (WOR-70 zinciri WFC-RETURN'ü de bir yazar olarak tanır).
#[test]
fn return_effects_count_as_a_context_writer() {
    assert!(!has_error(
        &local(caller_value()),
        "context_field_never_written"
    ));
}

// ---- ardıl (terminal) kısıtları ----

#[test]
fn next_call_rejects_return_fields() {
    for field in ["wfes_effects", "wft", "timeout"] {
        let v = mutate(|v| {
            v["terminals"][0]["call"][field] = match field {
                "wfes_effects" => json!({ "set": { "onay_tarihi": "$timestamp" } }),
                "wft" => json!({ "terminal": "terminal_rejected" }),
                _ => json!("P1D"),
            };
        });
        assert!(
            has_error(&local(v), "call_next_forbidden_field"),
            "ardıl çağrıda '{field}' reddedilmeli"
        );
    }
}

#[test]
fn node_only_fields_are_rejected_on_a_node_call() {
    for field in ["start_as", "max_next"] {
        let v = mutate(|v| {
            v["nodes"]["self__creditAnalyst"]["call"][field] = match field {
                "start_as" => json!("system"),
                _ => json!(3),
            };
        });
        assert!(
            has_error(&local(v), "call_node_forbidden_field"),
            "alt akış çağrısında '{field}' reddedilmeli"
        );
    }
}

/// Ardılda WFC-OUT yoktur: çağıran bitti, çağrılan henüz başlamadı.
#[test]
fn call_namespace_in_a_chained_terminal_is_rejected() {
    let v = mutate(|v| {
        v["terminals"][0]["wfes_effects"]["set"]["onay_tarihi"] = json!("$call.status");
    });
    assert!(has_error(&local(v), "call_next_result_ref"));
}

/// Kök `timeout` varsa terminal'e zaman aşımıyla da ulaşılabilir; o yolda aktör yok.
#[test]
fn start_as_actor_warns_when_root_timeout_exists() {
    let v = mutate(|v| {
        v["terminals"][0]["call"]["start_as"] = json!("actor");
    });
    assert!(has_warning(&local(v), "call_next_start_actor"));
}

#[test]
fn start_as_actor_is_fine_without_root_timeout() {
    let v = mutate(|v| {
        v.as_object_mut().unwrap().remove("timeout");
        v["terminals"][0]["call"]["start_as"] = json!("actor");
    });
    assert!(!has_warning(&local(v), "call_next_start_actor"));
}

#[test]
fn max_next_zero_is_rejected() {
    let v = mutate(|v| {
        v["terminals"][0]["call"]["max_next"] = json!(0);
    });
    assert!(has_error(&local(v), "call_next_max"));
}

// ---- döngüler ----

#[test]
fn self_recursion_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["wfd_id"] = json!("akis-cagrisi");
    });
    assert!(has_error(&local(v), "call_self_recursion"));
}

/// Ardıl modda bilinçli döngü `max_next` ile açıkça istenebilir.
#[test]
fn self_chain_is_allowed_with_max_next() {
    let v = mutate(|v| {
        v["calls"]["kredi_kullandirim"]["wfd_id"] = json!("akis-cagrisi");
        v["terminals"][0]["call"]["max_next"] = json!(2);
    });
    assert!(!has_error(&local(v), "call_self_recursion"));
}

/// Dolaylı yuvalanma döngüsü: çağıran → kredi-skor → çağıran.
#[test]
fn indirect_nesting_cycle_is_rejected() {
    let mut skor: Value = serde_json::from_str(SKOR).unwrap();
    skor["calls"] = json!({
        "geri_cagir": {
            "wfd_id": "akis-cagrisi",
            "version": "1.0.0",
            "start": "start__type_branch__branchClerk",
            "input": { "basvuru": { "musteri_no": "$ctx.musteri_no" } }
        }
    });
    skor["nodes"]["self__riskAnalyst"]["call"] = json!({
        "use": "geri_cagir",
        "mode": "wait",
        "wft": { "terminal": "terminal_skor_dusuk" }
    });
    // Bu node artık WFC node'u — insan transition'ı kaldırılmalı.
    skor["transitions"] = json!([]);

    let catalog = Catalog(vec![
        Wfd::from_value(skor).unwrap(),
        Wfd::from_json(KULLANDIRIM).unwrap(),
    ]);
    assert!(has_error(&full_with(caller_value(), catalog), "call_cycle"));
}

/// Dolaylı ardıl döngüsü: çağıran biter → kullandırım biter → çağıran başlar.
/// `max_next` verilmediği için reddedilir.
#[test]
fn indirect_next_cycle_without_max_next_is_rejected() {
    let mut kullandirim: Value = serde_json::from_str(KULLANDIRIM).unwrap();
    kullandirim["calls"] = json!({
        "bastan_basla": {
            "wfd_id": "akis-cagrisi",
            "version": "1.0.0",
            "start": "start__type_branch__branchClerk",
            "input": { "basvuru": { "musteri_no": "$ctx.musteri_no" } }
        }
    });
    kullandirim["terminals"][0]["call"] = json!({
        "use": "bastan_basla",
        "mode": "terminal",
        "start_as": "system"
    });

    let catalog = Catalog(vec![
        Wfd::from_json(SKOR).unwrap(),
        Wfd::from_value(kullandirim).unwrap(),
    ]);
    assert!(has_error(
        &full_with(caller_value(), catalog),
        "call_next_cycle"
    ));
}

// ---- versiyon çözümü ----

#[test]
fn unresolvable_callee_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["wfd_id"] = json!("hic-olmayan-akis");
    });
    assert!(has_error(&full(v), "call_version_not_published"));
}

#[test]
fn unpublished_pinned_version_is_rejected() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]["version"] = json!("9.9.9");
    });
    assert!(has_error(&full(v), "call_version_not_published"));
}

/// `version` yoksa en son yayınlanmış sürüm kullanılır — pin'siz çağrı geçerlidir.
#[test]
fn unpinned_version_resolves_to_latest() {
    let v = mutate(|v| {
        v["calls"]["kredi_skor_sorgusu"]
            .as_object_mut()
            .unwrap()
            .remove("version");
    });
    assert!(!has_error(&full(v), "call_version_not_published"));
}
