//! WOR-23 kabul testleri — golden fixture v2.2 parse + anahtar biçimi + canonical c_a
//! tekilliği + versiyon kapısı.
//! Spec: docs/spec/migration-notes.md, runtime-semantics.md §2.

use wfe_core::types::wfd_v22::{COrgu, CandidateActor, Wfd};

const FIXTURE: &str = include_str!("fixtures/kredi-basvuru.golden.json");

fn golden() -> Wfd {
    Wfd::from_json(FIXTURE).expect("golden fixture v2.2 parse etmeli")
}

#[test]
fn golden_fixture_parses_losslessly() {
    let wfd = golden();
    assert_eq!(wfd.wfd_version, "2.2");
    assert_eq!(wfd.id, "kredi-basvuru-v2");
    assert_eq!(wfd.nodes.len(), 4); // 3 state + 1 simetrik start node (type_branch__branchClerk)
    assert_eq!(wfd.start.len(), 1);
    assert_eq!(wfd.transitions.len(), 2);
    assert_eq!(wfd.terminals.len(), 2);
    assert_eq!(wfd.listable.len(), 2);
    assert_eq!(wfd.autoexec.len(), 3);
    // escalation korunmalı
    assert_eq!(wfd.nodes["self__creditAnalyst"].escalation.len(), 1);
    assert_eq!(wfd.nodes["self__creditAnalyst"].escalation[0].after, "P3D");
    // trigger retry/catch korunmalı
    let t = &wfd.transitions[0];
    assert_eq!(t.trigger.len(), 3);
    assert_eq!(t.trigger[0].retry.len(), 1);
    assert!(t.trigger[0].catch.is_some());
    assert!(!t.trigger[2].required);
}

// NOT: "node key == slug(c_a)" kuralı 2026-08-12'de KALDIRILDI (bkz. validator.rs §2b
// notu) — kimliği artık tasarımcı verir. Bu fixture'ın anahtarları TARİHSEL olarak slug
// biçimindedir ve öyle kalır (geriye uyumluluk kanıtı: eski belgeler geçerliliğini korur),
// ama bu bir sözleşme DEĞİLDİR; kuralı doğrulayan iki test bilerek silindi.
//
// 2026-08-14: TEKİLLİK kısıtı `duplicate_c_a` HATASI olarak geri geldi ve `canonical()`
// üzerinden ölçülüyor — aşağıdaki canonical testi onun tabanını sabitler. Motorun
// `slug()` yardımcısı ise 2026-08-14'te SİLİNDİ: kimlik üretmiyordu ve tek çağıranı
// kendi testiydi. §2a slug algoritması editörün varsayılan anahtar önerisi olarak
// spec'te durur (`docs/spec/runtime-semantics.md` §2a + `reference-types.rs`).

#[test]
fn golden_node_keys_still_satisfy_the_schema_id_pattern() {
    // Kural kalktı ama BİÇİM kısıtı şemada duruyor (`nodes` propertyNames: idName).
    let wfd = golden();
    for key in wfd.nodes.keys() {
        let mut chars = key.chars();
        let first = chars.next().expect("node key boş olamaz");
        assert!(
            first.is_ascii_alphabetic() || first == '_',
            "node key '{key}' harf ya da '_' ile başlamalı"
        );
        assert!(
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
            "node key '{key}' desene uymuyor"
        );
    }
}

#[test]
fn unknown_wfd_version_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    v["wfd_version"] = serde_json::json!("3.0");
    let err = Wfd::from_value(v).unwrap_err();
    assert!(
        err.to_string().contains("3.0"),
        "hata bilinmeyen versiyonu söylemeli: {err}"
    );
}

#[test]
fn missing_wfd_version_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    v.as_object_mut().unwrap().remove("wfd_version");
    assert!(Wfd::from_value(v).is_err());
}

#[test]
fn unknown_root_field_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    v["surprise_field"] = serde_json::json!(1);
    assert!(
        Wfd::from_value(v).is_err(),
        "root'ta bilinmeyen alan reddedilmeli (M14)"
    );
}

#[test]
fn deprecated_multi_rule_c_a_array_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    // node c_a'sını eski array formuna çevir — v2.2 tek kural (obje) ister
    let ca = v["nodes"]["self__creditAnalyst"]["c_a"].clone();
    v["nodes"]["self__creditAnalyst"]["c_a"] = serde_json::json!([ca]);
    assert!(
        Wfd::from_value(v).is_err(),
        "eski çok-kurallı c_a array'i reddedilmeli (M10)"
    );
}

#[test]
fn legacy_inline_c_a_start_is_rejected() {
    // Eski simetrik-öncesi start: from/action yok, c_a inline → deny_unknown_fields + eksik alan
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let obj = v["start"][0].as_object_mut().unwrap();
    obj.remove("from");
    obj.remove("action");
    obj.insert(
        "c_a".into(),
        serde_json::json!({"c_orgu": "self", "c_r": ["branchClerk"]}),
    );
    assert!(
        Wfd::from_value(v).is_err(),
        "eski inline-c_a start reddedilmeli"
    );
}

#[test]
fn canonical_c_a_normalizes_order_but_separates_channels() {
    // `duplicate_c_a` (validator §2b, 2026-08-14) bu formla ölçülür — kimlik üretmez.
    let cu_only = CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: None,
        c_u: Some(vec!["user_ayse".into()]),
    };

    // Rol/kişi SIRASI normalize edilir: aynı havuz, aynı canonical → tek node.
    let roles_ab = CandidateActor {
        c_orgu: Some(COrgu::Selector("parent".into())),
        c_r: Some(vec!["alpha".into(), "zeta".into()]),
        c_u: None,
    };
    let roles_ba = CandidateActor {
        c_orgu: Some(COrgu::Selector("parent".into())),
        c_r: Some(vec!["zeta".into(), "alpha".into()]),
        c_u: None,
    };
    assert_eq!(roles_ab.canonical(), roles_ba.canonical());

    // Çapasız biçim, çapalı eşdeğeriyle AYNI c_a sayılmaz: `None` ≠ `Some("self")`,
    // yani ikisi ayrı node'da durabilir.
    let anchorless = CandidateActor {
        c_orgu: None,
        c_r: None,
        c_u: Some(vec!["user_ayse".into()]),
    };
    assert_ne!(anchorless.canonical(), cu_only.canonical());

    // Farklı kanallar (rol ↔ kişi) da ayrışır.
    let r_rule = CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: Some(vec!["user_ayse".into()]),
        c_u: None,
    };
    assert_ne!(r_rule.canonical(), cu_only.canonical());
}

#[test]
fn c_u_match_is_role_agnostic_and_missing_field_is_false() {
    let cu_rule = CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: None,
        c_u: Some(vec!["user_ayse".into()]),
    };
    // rol ne olursa olsun user eşleşir
    assert!(cu_rule.matches_identity("creditAnalyst", "user_ayse"));
    assert!(cu_rule.matches_identity("branchClerk", "user_ayse"));
    assert!(!cu_rule.matches_identity("creditAnalyst", "user_mehmet"));

    // c_r verilmemişse rol kanalı match üretmez (wildcard DEĞİL)
    let r_rule = CandidateActor {
        c_orgu: Some(COrgu::Selector("self".into())),
        c_r: Some(vec!["creditAnalyst".into()]),
        c_u: None,
    };
    assert!(r_rule.matches_identity("creditAnalyst", "user_x"));
    assert!(!r_rule.matches_identity("other", "user_x"));
}

#[test]
fn serializes_back_to_equivalent_json() {
    let wfd = golden();
    let round: serde_json::Value = serde_json::to_value(&wfd).unwrap();
    let orig: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    // Yapısal eşdeğerlik: yeniden parse edilebilmeli ve anahtar alanlar korunmalı
    let re = Wfd::from_value(round.clone()).expect("serialize edilen WFD tekrar parse edilmeli");
    assert_eq!(re.nodes.len(), wfd.nodes.len());
    assert_eq!(
        orig["nodes"]["self__creditAnalyst"]["c_a"],
        round["nodes"]["self__creditAnalyst"]["c_a"]
    );
    assert_eq!(orig["transitions"], round["transitions"]);
    assert_eq!(orig["terminals"], round["terminals"]);
}
