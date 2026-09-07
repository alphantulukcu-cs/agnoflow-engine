//! WF Admin — akış-içi yetkili (T‑A5, T‑A6).
//! Tasarım: docs/superpowers/specs/2026-08-11-wf-admin-design.md
//!
//! WF Admin İŞİ YÖNETİR, İŞİ YAPMAZ. 2026-08-21 (A-1): yetki ÖRTÜK DEĞİL, LİSTELİ —
//! kurala uymak yalnız GÖRME verir, her müdahale `allowed_global_actions`ta yazmak zorunda.

use serde_json::json;
use wfe_core::types::wfd_v22::{GlobalAction, Wfd};

/// `wf_admin` kökte durur ve `listable`ın şeklini GENİŞLETİR (`{c_a, when?}` +
/// `allowed_global_actions`).
#[test]
fn wf_admin_parses_from_root() {
    let wfd = wfd_with_admin(json!([
        { "c_a": { "c_orgu": "self", "c_r": ["genel-mudur"] } },
        { "c_a": { "c_u": ["ahmet"] }, "when": "$ctx.tutar > 100000" }
    ]));
    assert_eq!(wfd.wf_admin.len(), 2);
    assert_eq!(
        wfd.wf_admin[0].grant.c_a.c_r.as_deref(),
        Some(&["genel-mudur".to_string()][..])
    );
    assert_eq!(
        wfd.wf_admin[1].grant.when.as_deref(),
        Some("$ctx.tutar > 100000")
    );
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

/// Grant şekli `listable` ile paylaşıldığı için ikisi de aynı JSON'dan okunur —
/// `allowed_global_actions` YOKKEN belge birebir aynıdır.
#[test]
fn wf_admin_and_listable_share_the_rule_shape() {
    let rule = json!({ "c_a": { "c_orgu": "self", "c_r": ["mudur"] }, "when": "true" });
    let mut doc = base_doc();
    doc["wf_admin"] = json!([rule.clone()]);
    doc["listable"] = json!([rule]);
    let wfd = Wfd::from_value(doc).expect("parse");
    assert_eq!(wfd.wf_admin.len(), 1);
    assert_eq!(wfd.listable.len(), 1);
    assert_eq!(wfd.wf_admin[0].grant.when, wfd.listable[0].when);
    // Liste yazılmadıysa boştur: admin GÖRÜR ama hiçbir müdahaleye yetkili DEĞİLDİR.
    assert!(wfd.wf_admin[0].allowed_global_actions.is_empty());
}

/// A-1: `allowed_global_actions` okunur ve sıra korunur (küme birleşimi motorda).
#[test]
fn allowed_global_actions_parses() {
    let wfd = wfd_with_admin(json!([
        { "c_a": { "c_orgu": "self", "c_r": ["mudur"] },
          "allowed_global_actions": ["send_back", "cancel"] }
    ]));
    assert_eq!(
        wfd.wf_admin[0].allowed_global_actions,
        vec![GlobalAction::SendBack, GlobalAction::Cancel]
    );
}

/// Bilinmeyen global aksiyon = PARSE HATASI. Sessizce yok saymak, tasarımcının
/// yazdığı yetkinin yayına çıkıp hiç işlememesi demekti.
#[test]
fn unknown_global_action_is_rejected() {
    let mut doc = base_doc();
    doc["wf_admin"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["mudur"] },
          "allowed_global_actions": ["send_back", "delete_everything"] }
    ]);
    let err = Wfd::from_value(doc).expect_err("bilinmeyen aksiyon reddedilmeli");
    assert!(
        format!("{err:?}").contains("delete_everything"),
        "hata bilinmeyen aksiyonu adıyla söylemeli: {err:?}"
    );
}

/// Boş liste ile hiç yazılmamış liste AYNI anlamdadır (ikisi de "yalnız görme") ve
/// serileştirmede boş liste DÜŞER — belge iki şekilde yazılıp aynı yere varır.
#[test]
fn empty_allowed_global_actions_is_not_serialized() {
    let wfd = wfd_with_admin(json!([
        { "c_a": { "c_orgu": "self", "c_r": ["mudur"] }, "allowed_global_actions": [] }
    ]));
    assert!(wfd.wf_admin[0].allowed_global_actions.is_empty());
    let round = serde_json::to_value(&wfd).expect("serialize");
    assert!(
        round["wf_admin"][0].get("allowed_global_actions").is_none(),
        "boş liste serileşmemeli: {}",
        round["wf_admin"][0]
    );
}

// ── yardımcılar ─────────────────────────────────────────────────────────────

const FIXTURE: &str = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");

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

/// Kural içinde bilinmeyen alan REDDEDİLİR — `flatten` ile gömülü `CaGrantRule`ın
/// `deny_unknown_fields`ı hâlâ işliyor mu? (Tasarımcının yazım hatası sessizce
/// yutulursa yetki hiç işlemez ve sebebi görünmez.)
#[test]
fn unknown_field_in_wf_admin_rule_is_rejected() {
    let mut doc = base_doc();
    doc["wf_admin"] = json!([
        { "c_a": { "c_orgu": "self", "c_r": ["mudur"] }, "allowed_globl_actions": ["cancel"] }
    ]);
    let err = Wfd::from_value(doc).expect_err("bilinmeyen alan reddedilmeli");
    assert!(
        format!("{err:?}").contains("allowed_globl_actions"),
        "hata alanı adıyla söylemeli: {err:?}"
    );
}
