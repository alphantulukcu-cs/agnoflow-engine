//! `docs/spec/schema.json` → `x-dollar-env` tablosu ile motorun `$`-STRING GRAMERİ
//! (`v22::dollar`) aynı şeyi anlatmalı (`S25`/`WOR-116`).
//!
//! ## Neden bu dosya var
//!
//! Editör bu grameri **elle** aynalıyordu ve üstelik İKİ ayrı listede:
//! `DOLLAR_EXACT`/`DOLLAR_PREFIXES` (renklendirme + `classifyDollar`) ve
//! `SURFACE_NAMESPACES` sabitleri (öneri listesi). Üçü de ölçüldüğünde uyuşuyordu
//! (2026-09-09) ama uyuşmayı SÜRDÜREN bir mekanizma yoktu.
//!
//! Ayrışmanın bedeli burada `zen_unknown_root`unkinden farklı ve daha sinsi: motor
//! tanımadığı `$`-string'i **hata saymaz**, alana o METNİ yazar
//! (`effects::resolve_dollar_string` son satırı). Yani editör var olmayan bir namespace
//! önerirse tasarımcı `"$call.state"` yazar, alan o metni alır, o alanı okuyan koşullar
//! sessizce hep-false olur ve log da çıkmaz. Tasarım zamanı reddi
//! (`unknown_dollar_ref`) tam bu yüzden var — ve o red MOTORUN tablosuna bakıyor.
//!
//! `D02`nin `x-zen-env` için kurduğu desenin aynısı: gramer spec'te veri, motorda
//! parite kapısı, editörde türetim. Emsal `zen_env_parity.rs` ve
//! `reference_types_parity.rs`.
//!
//! ## ⚠️ `x-zen-env` ile KARIŞTIRILMAZ
//!
//! İkisi AYRI gramerdir ve kesişmeleri yanıltıcıdır:
//!
//! * ZEN'in kökleri (`$wfah`, `$valid`, `$prev`, `$first`, `$branches`, `$arrived`,
//!   `$branch_round`) bir `$`-string yüzeyinde **ÇÖZÜLMEZ**;
//! * buradaki `$call.result.` ZEN'de bir KÖK değil, `$call`ın üyesidir;
//! * `$env` ikisinde de var ama kuralı burada farklı (ara-değer olarak da çözülür).
//!
//! Bu yüzden `WOR-116`nın kaydındaki *"motorun `ZEN_ROOTS` tablosuyla eşleşsin"*
//! ifadesi yanlış kaynağı gösteriyordu; doğru kaynak `v22::dollar`dır.

use std::collections::BTreeSet;

use serde_json::Value;
use wfe_core::v22::dollar::{EXACT, PREFIXES};

const SCHEMA_SRC: &str = include_str!("../../../docs/spec/schema.json");

fn dollar_env() -> Value {
    let schema: Value = serde_json::from_str(SCHEMA_SRC).expect("schema.json parse etmeli");
    schema
        .get("x-dollar-env")
        .cloned()
        .expect("schema.json `x-dollar-env` tablosunu TAŞIMALI (S25) — editör onu okuyor")
}

fn list(env: &Value, path: &[&str]) -> BTreeSet<String> {
    let mut node = env;
    for key in path {
        node = node
            .get(key)
            .unwrap_or_else(|| panic!("x-dollar-env.{} KAYIP", path.join(".")));
    }
    node.as_array()
        .unwrap_or_else(|| panic!("x-dollar-env.{} bir dizi olmalı", path.join(".")))
        .iter()
        .map(|v| v.as_str().expect("dizi elemanı string olmalı").to_string())
        .collect()
}

/// Tam eşleşen referanslar birebir aynı olmalı.
#[test]
fn exact_references_match_the_engine() {
    let engine: BTreeSet<String> = EXACT.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        list(&dollar_env(), &["exact"]),
        engine,
        "`x-dollar-env.exact` motorun `dollar::EXACT` tablosuyla AYRIŞTI"
    );
}

/// Yol taşıyan önekler: spec CANLI olanları sayar, motor tablosunda ÖLÜ olanlar da
/// durur. Bu yüzden karşılaştırma `canlı ∪ ölü == motor` üzerinden yapılır — ölüyü
/// spec'ten düşürmek onu sessizce canlı listeye geri sokardı.
#[test]
fn prefixes_match_the_engine_including_the_dead_one() {
    let env = dollar_env();
    let engine: BTreeSet<String> = PREFIXES.iter().map(|s| s.to_string()).collect();
    let live = list(&env, &["prefixes"]);
    let dead = list(&env, &["deadPrefixes", "items"]);

    assert!(
        live.is_disjoint(&dead),
        "bir önek hem canlı hem ölü listede: {:?}",
        live.intersection(&dead).collect::<Vec<_>>()
    );
    assert_eq!(
        live.union(&dead).cloned().collect::<BTreeSet<_>>(),
        engine,
        "`x-dollar-env` önekleri motorun `dollar::PREFIXES` tablosuyla AYRIŞTI"
    );
    assert!(
        !dead.is_empty(),
        "`deadPrefixes` boşaldı — motorda hâlâ duran `$exec.response.` canlı sanılır"
    );
}

/// `$env` İKİ listenin de DIŞINDA durur. Motorun tablosunda da yok: kuralı ayrı
/// (`check_env_refs`) ve ara-değer olarak da çözülüyor — önek listesine girseydi
/// tüketici onu "tam bir referans olmalı" diye ele alırdı.
#[test]
fn env_is_outside_both_lists() {
    let env = dollar_env();
    assert_eq!(env["env"]["prefix"].as_str(), Some("$env."));
    assert!(
        !EXACT.contains(&"$env.") && !PREFIXES.contains(&"$env."),
        "motorun `$`-tablosuna `$env.` sızmış — kuralı ayrı olmalı"
    );
    assert!(!list(&env, &["prefixes"]).contains("$env."));
    assert!(!list(&env, &["exact"]).contains("$env."));
}
