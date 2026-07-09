//! WOR-23 kabul testleri — golden fixture v2.2 parse + slug + uniqueness + versiyon kapısı.
//! Spec: docs/spec/WFD_MIGRATION_NOTES_v2_2.md, wfd-custom-validator-runtime-semantics_v2_2.md §2.

use wfe_core::types::wfd_v22::{CandidateActor, COrgu, Wfd};

const FIXTURE: &str = include_str!("fixtures/example-wfd_kredi-basvuru_v2_2.json");

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

#[test]
fn every_node_key_equals_slug_of_its_c_a() {
    let wfd = golden();
    for (key, node) in &wfd.nodes {
        assert_eq!(
            key,
            &node.c_a.slug(),
            "node key '{key}' c_a slug'ı ile eşleşmeli"
        );
    }
}

#[test]
fn canonical_c_a_is_unique_across_nodes() {
    let wfd = golden();
    let mut seen = std::collections::HashSet::new();
    for node in wfd.nodes.values() {
        assert!(
            seen.insert(node.c_a.canonical()),
            "aynı canonical c_a iki node'da bulunamaz"
        );
    }
}

#[test]
fn unknown_wfd_version_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    v["wfd_version"] = serde_json::json!("3.0");
    let err = Wfd::from_value(v).unwrap_err();
    assert!(err.to_string().contains("3.0"), "hata bilinmeyen versiyonu söylemeli: {err}");
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
    assert!(Wfd::from_value(v).is_err(), "root'ta bilinmeyen alan reddedilmeli (M14)");
}

#[test]
fn deprecated_multi_rule_c_a_array_is_rejected() {
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    // node c_a'sını eski array formuna çevir — v2.2 tek kural (obje) ister
    let ca = v["nodes"]["self__creditAnalyst"]["c_a"].clone();
    v["nodes"]["self__creditAnalyst"]["c_a"] = serde_json::json!([ca]);
    assert!(Wfd::from_value(v).is_err(), "eski çok-kurallı c_a array'i reddedilmeli (M10)");
}

#[test]
fn legacy_inline_c_a_start_is_rejected() {
    // Eski simetrik-öncesi start: from/action yok, c_a inline → deny_unknown_fields + eksik alan
    let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let obj = v["start"][0].as_object_mut().unwrap();
    obj.remove("from");
    obj.remove("action");
    obj.insert("c_a".into(), serde_json::json!({"c_orgu": "self", "c_r": ["branchClerk"]}));
    assert!(Wfd::from_value(v).is_err(), "eski inline-c_a start reddedilmeli");
}

#[test]
fn slug_algorithm_matches_spec_examples() {
    // runtime-semantics §2a örnekleri
    let simple = CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: Some(vec!["creditAnalyst".into()]),
        c_u: None,
    };
    assert_eq!(simple.slug(), "self__creditAnalyst");

    let typed = CandidateActor {
        c_orgu: COrgu::Selector("*:[type:branch]".into()),
        c_r: Some(vec!["branchClerk".into()]),
        c_u: None,
    };
    assert_eq!(typed.slug(), "type_branch__branchClerk");

    let cu_only = CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: None,
        c_u: Some(vec!["user_ayse".into()]),
    };
    assert_eq!(cu_only.slug(), "self__u_user_ayse");

    // roller sıralanır, deterministiktir
    let two_roles = CandidateActor {
        c_orgu: COrgu::Selector("parent".into()),
        c_r: Some(vec!["zeta".into(), "alpha".into()]),
        c_u: None,
    };
    assert_eq!(two_roles.slug(), "parent__alpha-zeta");
}

#[test]
fn c_u_match_is_role_agnostic_and_missing_field_is_false() {
    let cu_rule = CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
        c_r: None,
        c_u: Some(vec!["user_ayse".into()]),
    };
    // rol ne olursa olsun user eşleşir
    assert!(cu_rule.matches_identity("creditAnalyst", "user_ayse"));
    assert!(cu_rule.matches_identity("branchClerk", "user_ayse"));
    assert!(!cu_rule.matches_identity("creditAnalyst", "user_mehmet"));

    // c_r verilmemişse rol kanalı match üretmez (wildcard DEĞİL)
    let r_rule = CandidateActor {
        c_orgu: COrgu::Selector("self".into()),
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
    assert_eq!(orig["nodes"]["self__creditAnalyst"]["c_a"], round["nodes"]["self__creditAnalyst"]["c_a"]);
    assert_eq!(orig["transitions"], round["transitions"]);
    assert_eq!(orig["terminals"], round["terminals"]);
}
