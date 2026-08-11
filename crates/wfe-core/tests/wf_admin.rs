//! WF Admin — akış-içi yetkili (T‑A5, T‑A6).
//! Tasarım: docs/superpowers/specs/2026-08-11-wf-admin-design.md
//!
//! WF Admin İŞİ YÖNETİR, İŞİ YAPMAZ: devir + escalation sayacı; aksiyon yetkisi vermez.

use serde_json::json;
use wfe_core::types::wfd_v22::Wfd;

/// `wf_admin` kökte durur ve `listable` ile AYNI şekli taşır (`{c_a, when?}`).
#[test]
fn wf_admin_parses_from_root() {
    let wfd = wfd_with_admin(json!([
        { "c_a": { "c_orgu": "self", "c_r": ["genel-mudur"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.tutar > 100000" }
    ]));
    assert_eq!(wfd.wf_admin.len(), 2);
    assert_eq!(wfd.wf_admin[0].c_a.c_r.as_deref(), Some(&["genel-mudur".to_string()][..]));
    assert_eq!(wfd.wf_admin[1].when.as_deref(), Some("$ctx.tutar > 100000"));
}

/// Alan verilmezse boştur ve yeniden serileştirmede HİÇ çıkmaz — `wf_admin`
/// taşımayan belgeler birebir aynı serileşir (golden fixture korunur).
#[test]
fn missing_wf_admin_is_empty_and_not_serialized() {
    let wfd = wfd_with_admin(json!(null));
    assert!(wfd.wf_admin.is_empty());
    let round = serde_json::to_value(&wfd).expect("serialize");
    assert!(
        round.get("wf_admin").is_none(),
        "boş wf_admin serileştirmede görünmemeli: {round}"
    );
}

/// Kural şekli `listable` ile paylaşıldığı için ikisi de aynı JSON'dan okunur.
#[test]
fn wf_admin_and_listable_share_the_rule_shape() {
    let rule = json!({ "c_a": { "c_orgu": "self", "c_r": ["mudur"] }, "when": "true" });
    let mut doc = base_doc();
    doc["wf_admin"] = json!([rule.clone()]);
    doc["listable"] = json!([rule]);
    let wfd = Wfd::from_value(doc).expect("parse");
    assert_eq!(wfd.wf_admin.len(), 1);
    assert_eq!(wfd.listable.len(), 1);
    assert_eq!(wfd.wf_admin[0].when, wfd.listable[0].when);
}

// ── yardımcılar ─────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");

fn wfd_with_admin(wf_admin: serde_json::Value) -> Wfd {
    let mut doc = base_doc();
    if !wf_admin.is_null() {
        doc["wf_admin"] = wf_admin;
    }
    Wfd::from_value(doc).expect("parse")
}

/// Kanonik golden belge. Elle kurulmuş minimal bir doküman yerine bunu kullanmak,
/// testin gerçek bir v2.2 belgesinin tüm zorunlu alanlarıyla koşmasını garanti eder.
fn base_doc() -> serde_json::Value {
    serde_json::from_str(FIXTURE).expect("fixture JSON")
}

// ── şema kapısı (docs/spec/schema.json) ─────────────────────────────────────
// Kök `additionalProperties: false` — şemaya eklenmemiş olsaydı GEÇERLİ bir wf_admin
// taşıyan belge de reddedilirdi. `from_value` şemayı koşmaz, `from_value_checked` koşar.

#[test]
fn schema_gate_accepts_valid_wf_admin() {
    let mut doc = base_doc();
    doc["wf_admin"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["branchManager"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.x > 1" }
    ]);
    Wfd::from_value_checked(doc).expect("geçerli wf_admin şema kapısından geçmeli");
}

#[test]
fn schema_gate_rejects_unknown_field_in_rule() {
    let mut doc = base_doc();
    doc["wf_admin"] = json!([{ "c_a": { "c_orgu": "self", "c_r": ["x"] }, "sarkan": true }]);
    assert!(
        Wfd::from_value_checked(doc).is_err(),
        "kural içinde bilinmeyen alan reddedilmeli (additionalProperties: false)"
    );
}

#[test]
fn schema_gate_rejects_empty_c_r_in_wf_admin() {
    // `"c_r": []` serde için geçerli görünür (boş Vec) — kapı ŞEMADIR (minItems).
    let mut doc = base_doc();
    doc["wf_admin"] = json!([{ "c_a": { "c_orgu": "self", "c_r": [] } }]);
    assert!(
        Wfd::from_value_checked(doc).is_err(),
        "boş c_r reddedilmeli (candidateActor $ref'i wf_admin'de de işlemeli)"
    );
}

#[test]
fn schema_gate_requires_c_a_in_rule() {
    let mut doc = base_doc();
    doc["wf_admin"] = json!([{ "when": "true" }]);
    assert!(
        Wfd::from_value_checked(doc).is_err(),
        "c_a olmadan kural olamaz"
    );
}
