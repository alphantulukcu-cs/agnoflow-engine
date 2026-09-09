//! `docs/spec/schema.json` → `x-zen-env` tablosu ile motorun ZEN ORTAMI aynı şeyi
//! anlatmalı (`D02`/S5).
//!
//! ## Neden bu dosya var
//!
//! Editör kök listesini ve `$wfah` satır alanlarını **elle** yazıyordu ve ölçüldüğünde
//! motordan DÖRT ad geride kalmıştı: `$call.*`, `$branches`, `$arrived` ZEN
//! önerilerinde hiç yoktu, `$valid` de beşinci olacaktı. Sapma görünmedi çünkü
//! görecek bir mekanizma yoktu — motor tarafında unutulan kök `zen_unknown_root` ile
//! **gürültülü** hata verir, editörde unutulan kök ise **sessizdir**: tasarımcı meşru
//! bir ifadeyi yazamaz ve kimse öğrenmez.
//!
//! `D02` bu yüzden tabloyu spec'e veri olarak yazdırdı ve tek cümlelik hükmünü koydu:
//! **"kapısız ayna yazılmaz."** Kapı budur. Editör tabloyu build zamanında okur
//! (`agnoflow-frontend` `src/common/utils/zenEnv.ts`); burada da tablonun MOTORUN
//! ortamıyla birebir olduğu doğrulanır. Ayrışma iki yönde de kırılır:
//!
//! * ortama kök eklenip tabloya yazılmazsa → editör onu hiç önermez (sessiz eksik),
//! * tabloya kök yazılıp ortamda yoksa → editör var olmayan bir kök vaat eder ve
//!   ifade yayında `zen_unknown_root` ile reddedilir.
//!
//! Emsal `reference_types_parity.rs`. Aynı gerekçeyle `kinds` listesi de KAYNAK
//! METİNDEN çıkarılır: `WfahKind` varyantlarını çalışma anında sayabilmenin yolu yok
//! (`strum` bağımlılığı için sebep yeterli değil), ama enum'a varyant eklemek
//! `wfah_kind.rs` metnini DEĞİŞTİRİR ve kapı o an kırılır.
//!
//! ## Kapsam DIŞI
//!
//! **Taşıyıcı yerin kendi daralması.** `set_when_namespace`, `sla_effect_namespace` ve
//! `call_effect_namespace` tablonun ÜSTÜNE biner; `x-zen-env` BAĞLAM düzeyindedir
//! (`zenAction` / `zenStart` / `zenGuard`), yer düzeyinde değil. O daralmanın editöre
//! ulaşması ayrı bir iştir ve bu kapı onu denetlemez.

use std::collections::BTreeSet;

use serde_json::Value;
use wfe_core::expr_types::{VALID_ONLY_SCALARS, WFAH_SCALARS, WFAH_TIMESTAMP_FIELDS};
use wfe_core::v22::eval::ZEN_ROOTS;
use wfe_core::validator::START_WHEN_ROOTS;

const SCHEMA_SRC: &str = include_str!("../../../docs/spec/schema.json");
const WFAH_KIND_SRC: &str = include_str!("../src/v22/wfah_kind.rs");

fn zen_env() -> Value {
    let schema: Value = serde_json::from_str(SCHEMA_SRC).expect("schema.json parse etmeli");
    schema
        .get("x-zen-env")
        .cloned()
        .expect("schema.json `x-zen-env` tablosunu TAŞIMALI (D02) — editör onu okuyor")
}

/// Bir bağlamda serbest kök adları.
fn roots_in(env: &Value, context: &str) -> BTreeSet<String> {
    env["roots"]
        .as_object()
        .expect("x-zen-env.roots bir obje olmalı")
        .iter()
        .filter(|(_, spec)| {
            spec["contexts"]
                .as_array()
                .expect("roots[].contexts bir dizi olmalı")
                .iter()
                .any(|c| c.as_str() == Some(context))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Satır alanı tablosunun bir listedeki hâli: `name -> type`, `actor` üç yola AÇILMIŞ,
/// `input` DIŞARIDA.
///
/// Motorun tablosu (`WFAH_SCALARS`) yolları düz tutar (`actor.role`); spec tablosu
/// alanı bir kez yazıp üyelerini `members` ile bildirir. Karşılaştırma motorun
/// şeklinde yapılır — açılım burada. `input` ise motorun skaler tablosunda hiç
/// YOKTUR (ağacı belgeden çözülür), o yüzden her iki tarafta da dışarıda kalır.
fn spec_fields(env: &Value, list: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for (name, spec) in env["entryFields"]
        .as_object()
        .expect("x-zen-env.entryFields bir obje olmalı")
    {
        let in_list = spec["lists"]
            .as_array()
            .expect("entryFields[].lists bir dizi olmalı")
            .iter()
            .any(|l| l.as_str() == Some(list));
        if !in_list {
            continue;
        }
        let ty = spec["type"].as_str().expect("entryFields[].type dize olmalı");
        match spec.get("members").and_then(Value::as_array) {
            Some(members) => {
                let member_ty = spec["memberType"]
                    .as_str()
                    .expect("`members` taşıyan alan `memberType` de taşımalı");
                for m in members {
                    let m = m.as_str().expect("members[] dize olmalı");
                    out.insert((format!("{name}.{m}"), member_ty.to_string()));
                }
            }
            // Açık ağaç (`input`): motorun skaler tablosunda karşılığı yok.
            None if spec.get("open").and_then(Value::as_bool) == Some(true) => {}
            None => {
                out.insert((name.to_string(), ty.to_string()));
            }
        }
    }
    out
}

fn engine_fields(with_valid_only: bool) -> BTreeSet<(String, String)> {
    WFAH_SCALARS
        .iter()
        .chain(if with_valid_only { VALID_ONLY_SCALARS } else { &[] }.iter())
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .collect()
}

/// `pub enum WfahKind { … }` gövdesindeki varyant adları, `serde(rename_all =
/// "snake_case")` karşılıklarına çevrilmiş hâlde.
fn engine_kinds() -> Vec<String> {
    let body = WFAH_KIND_SRC
        .split_once("pub enum WfahKind {")
        .expect("`pub enum WfahKind {` bulunamadı — enum yeniden adlandırıldıysa BU TEST güncellenir")
        .1
        .split_once("\n}")
        .expect("enum gövdesi kapanmıyor")
        .0;

    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with('#'))
        .map(|l| l.trim_end_matches(',').trim())
        .filter(|l| l.chars().next().is_some_and(char::is_uppercase))
        .map(to_snake_case)
        .collect()
}

fn to_snake_case(camel: &str) -> String {
    let mut out = String::new();
    for (i, c) in camel.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// `zenAction` ve `zenGuard` TAM tabloyu görür — `$node` dahil (`E11` + `E13`).
#[test]
fn action_and_guard_contexts_see_the_whole_evaluation_environment() {
    let env = zen_env();
    let declared: BTreeSet<String> = ZEN_ROOTS.iter().map(|r| r.to_string()).collect();

    assert_eq!(roots_in(&env, "zenAction"), declared, "zenAction kök kümesi");
    assert_eq!(roots_in(&env, "zenGuard"), declared, "zenGuard kök kümesi");
    assert_eq!(declared.len(), 16, "kök tablosu 16 köktür (E05/BAĞLI KURAL 2 + E14)");
}

/// `zenStart` DAR: `start_when_namespace`in beyaz listesi + `$action.input`.
///
/// Motor tarafında `$action` kökü yalnız `.input` alt yoluyla serbesttir; tablo kök
/// düzeyinde yazıldığı için `$action` orada TEK ad olarak görünür.
#[test]
fn start_context_matches_the_start_when_whitelist() {
    let env = zen_env();
    let mut declared: BTreeSet<String> = START_WHEN_ROOTS.iter().map(|r| r.to_string()).collect();
    declared.insert("$action".to_string());

    assert_eq!(roots_in(&env, "zenStart"), declared, "zenStart kök kümesi");
    assert_eq!(declared.len(), 5, "start `when`inde BEŞ kök serbesttir (E11/S1)");
}

/// Satır alanları: `$wfah` **11**, `$valid` **13** (`D02`/S2). Sayım `actor` ve
/// `input`u BİRER alan sayar; motorla karşılaştırma açılmış yollar üzerinde.
#[test]
fn entry_fields_match_the_engine_projection() {
    let env = zen_env();

    assert_eq!(spec_fields(&env, "wfah"), engine_fields(false), "$wfah satır alanları");
    assert_eq!(spec_fields(&env, "valid"), engine_fields(true), "$valid satır alanları");

    let count = |list: &str| {
        env["entryFields"]
            .as_object()
            .unwrap()
            .values()
            .filter(|f| f["lists"].as_array().unwrap().iter().any(|l| l.as_str() == Some(list)))
            .count()
    };
    assert_eq!(count("wfah"), 11, "$wfah 11 alan sunar");
    assert_eq!(count("valid"), 13, "$valid 13 alan sunar");
}

/// Hesaplanan iki alan YALNIZ `$valid`te: ham listede yazılırsa motor
/// `zen_wfah_field_unknown` HATASI verir, dolayısıyla editör onları `$wfah`ta
/// SUNMAMALI.
#[test]
fn computed_fields_are_valid_only() {
    let env = zen_env();
    for (name, _) in VALID_ONLY_SCALARS {
        let lists = env["entryFields"][name]["lists"]
            .as_array()
            .unwrap_or_else(|| panic!("x-zen-env.entryFields['{name}'] KAYIP"));
        assert_eq!(
            lists.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            vec!["valid"],
            "'{name}' yalnız $valid listesinde olmalı",
        );
    }
}

/// `#.kind`in kapalı listesi `WfahKind` varyantlarıyla BİREBİR — sıra dahil.
///
/// Sıra da denetlenir: liste editörde bir dropdown olarak çizilir ve motorun
/// bildirim sırası tasarımcının gördüğü sıradır (rastgele alfabetik değil).
#[test]
fn kind_enum_matches_the_engine_variants() {
    let env = zen_env();
    let spec: Vec<String> = env["kinds"]
        .as_array()
        .expect("x-zen-env.kinds bir dizi olmalı")
        .iter()
        .map(|k| k.as_str().expect("kinds[] dize olmalı").to_string())
        .collect();

    assert_eq!(spec, engine_kinds(), "#.kind kapalı listesi");
    assert_eq!(spec.len(), 15, "WfahKind 15 varyanttır (E07/S1)");
}

/// Zaman damgası alanları — biçim sabittir (`yyyyMMddHHmmss`) ve editör
/// `isTimestampLiteral` kapısını bu kümeye uygular.
#[test]
fn timestamp_fields_match_the_engine_table() {
    let env = zen_env();
    let spec: BTreeSet<&str> = env["entryFields"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, f)| f.get("timestamp").and_then(Value::as_bool) == Some(true))
        .map(|(name, _)| name.as_str())
        .collect();

    assert_eq!(spec, WFAH_TIMESTAMP_FIELDS.iter().copied().collect::<BTreeSet<_>>());
}

/// Tablo YALNIZ üç bağlam tanır. Dördüncüsü eklenirse editörün `ZenCtxKind` birliği
/// ve buradaki iki test birlikte güncellenmek zorundadır — sessizce sarkmasın.
///
/// KÜME olarak karşılaştırılır: `serde_json` obje anahtarlarını sıralı tutmuyor
/// (`preserve_order` açık değil), dolayısıyla dosyadaki yazım sırası burada bir
/// dayanak DEĞİL. `kinds` bir DİZİ olduğu için orada sıra denetlenebiliyor.
#[test]
fn only_three_contexts_are_declared() {
    let env = zen_env();
    let names: BTreeSet<&str> = env["contexts"]
        .as_object()
        .expect("x-zen-env.contexts bir obje olmalı")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, BTreeSet::from(["zenAction", "zenStart", "zenGuard"]));
}
