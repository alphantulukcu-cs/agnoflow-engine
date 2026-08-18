//! `docs/spec/reference-types.rs` ile motorun modeli AYNI ŞEYİ anlatmalı.
//!
//! ## Neden bu dosya var
//!
//! `reference-types.rs` spec'in "Rust tarafı nasıl modellenir?" cevabıdır: `README.md`,
//! `migration-notes.md`, `runtime-semantics.md` ve editörün `wfd-v22.types.ts`'i ona atıf
//! yapar. Ama `docs/` altında durduğu için HİÇBİR derleyici ona bakmıyordu — 2026-08-17'de
//! ölçüldüğünde 8 tip (`CallDef`/`CallRef`/`CallMode`/`StartAs`/`CuItem`/`GlobalTarget`/
//! `CaGrantRule`) ve 10'dan fazla alan geride kalmıştı, üstelik `c_u`'nun ŞEKLİ de
//! eskiydi (`Vec<String>`, oysa motor `Vec<CuItem>`). Kimse fark etmedi çünkü fark
//! edecek bir mekanizma yoktu.
//!
//! Kapı İKİ katmanlıdır, çünkü tek başına ikisi de yetmiyor:
//!
//! 1. **DERLENİR** (`#[path]` ile modül olarak alınır). Tip rotunu (var olmayan tipe
//!    referans, bozulmuş imza) derleyici yakalar.
//! 2. **ALAN PARİTESİ** (kaynak metinden tip/alan kümesi çıkarılıp karşılaştırılır).
//!    Derleme, motora EKLENEN ve referansa EKLENMEYEN bir alanı yakalayamaz — iki ayrı
//!    tip ağacıdır, biri diğerinden habersiz derlenir. Asıl çürüme de bu yönde oluyor.
//!
//! Fixture parse'ı (3. bir kapı gibi görünen şey) BİLEREK tek başına dayanak sayılmadı:
//! `deny_unknown_fields` yalnız fixture'da GEÇEN alanı yakalar, golden fixture ise
//! opsiyonel alanların çoğunu (`terminals[].listable`, `calls`, `wf_admin`) taşımaz.
//! Yine de koşuyor — ucuz ve gerçek belgeyle çalıştığını gösteriyor.
//!
//! ## Parite kuralı: motor ÜST KÜMEDİR
//!
//! Motorda olup referansta olmayan tip/alan HATADIR. Tersi hata DEĞİLDİR: referans
//! bilerek fazladan bir şey taşır — `CandidateActor::slug` (§2a), yani editörün yeni
//! node'a önereceği varsayılan anahtar. Motor onu 2026-08-14'te SİLDİ (orada kimlik
//! üretmiyordu, çağıranı yoktu); spec'te durması bilinçli.

#[path = "../../../docs/spec/reference-types.rs"]
mod reference;

use std::collections::{BTreeMap, BTreeSet};

const ENGINE_SRC: &str = include_str!("../src/types/wfd_v22.rs");
const REFERENCE_SRC: &str = include_str!("../../../docs/spec/reference-types.rs");

/// Referansta BİLEREK bulunmayan tipler — her biri için gerekçe zorunludur.
///
/// Listeye ekleme yapmak, "bu tip spec'te modellenmiyor" demektir; sessiz bir atlama
/// değil, yazılı bir karar olsun diye buradadır.
const ENGINE_ONLY: &[(&str, &str)] = &[
    // Çalışma-anı tipi: fork'ta ÇÖZÜLEN join kuralı `wf.wfe` satırında persist edilir
    // (`join_threshold`/`join_when` kolonlarından okunur). Belgede karşılığı
    // `ParallelSpec.join_mode` + `join_threshold` + `join_when`tir; ikisini aynı ada
    // koymak "belgede ne yazıyor" ile "satırda ne duruyor"u karıştırırdı.
    ("JoinRule", "runtime tipi — belgede değil wfe satırında yaşar"),
];

fn main_types(src: &str) -> BTreeMap<String, BTreeSet<String>> {
    let src = strip_comments(src);
    let mut out = BTreeMap::new();
    let mut idx = 0usize;
    while let Some(pos) = find_decl(&src, idx) {
        let (kind, name, after) = pos;
        idx = after;
        let Some(open) = src[after..].find('{').map(|i| after + i) else {
            continue;
        };
        // Gövde `{` ile `}` arasında — iç içe süslüler sayılır.
        let mut depth = 0usize;
        let mut end = open;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &src[open + 1..end];
        let members = if kind == "struct" {
            struct_fields(body)
        } else {
            enum_variants(body)
        };
        out.insert(name, members);
    }
    out
}

/// `pub struct X` / `pub enum X` bildirimini bulur; `(kind, name, bildirimden_sonraki_ofset)`.
fn find_decl(src: &str, from: usize) -> Option<(String, String, usize)> {
    for kw in ["pub struct ", "pub enum "] {
        if let Some(i) = src[from..].find(kw) {
            let start = from + i + kw.len();
            let name: String = src[start..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            // En yakın bildirimi seç (iki anahtar kelimeden hangisi önce geliyorsa).
            let other = ["pub struct ", "pub enum "]
                .iter()
                .filter(|k| **k != kw)
                .filter_map(|k| src[from..].find(k))
                .min();
            if let Some(o) = other {
                if o < i {
                    continue;
                }
            }
            let kind = if kw.contains("struct") { "struct" } else { "enum" };
            let end = start + name.len();
            return Some((kind.into(), name, end));
        }
    }
    None
}

fn struct_fields(body: &str) -> BTreeSet<String> {
    body.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("pub ")?;
            let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            // `pub fn` / `pub type` alan değildir; alanın ardından `:` gelir.
            let after = rest[name.len()..].trim_start();
            (after.starts_with(':') && !name.is_empty()).then_some(name)
        })
        .collect()
}

/// Enum varyantları — gövde DEPTH-0 virgülleriyle parçalanır.
///
/// Satır satır okumak yetmiyordu: `pub enum AutoexecType { Rest, Sql, Calc, Python,
/// Lambda }` gibi TEK SATIRLIK bir enum'da yalnız ilk varyant görülüyor ve testin
/// kendisi sessizce kör kalıyordu (tam da yakalaması gereken drift türü). Virgül
/// tabanlı bölme hem tek satırı hem çok satırlı tuple/struct varyantlarını kapsar.
fn enum_variants(body: &str) -> BTreeSet<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    for c in body.chars() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                segments.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    segments.push(current);

    segments
        .iter()
        .filter_map(|seg| {
            // Varyantın önündeki `#[default]` gibi öznitelik satırları atılır.
            let head = seg
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))?;
            let name: String = head.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            (!name.is_empty() && name.starts_with(char::is_uppercase)).then_some(name)
        })
        .collect()
}

/// Satır yorumlarını düşürür — `// pub foo:` gibi örnek metinler alan sayılmasın.
fn strip_comments(src: &str) -> String {
    src.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn reference_model_declares_every_engine_type() {
    let engine = main_types(ENGINE_SRC);
    let reference = main_types(REFERENCE_SRC);
    let excused: BTreeSet<&str> = ENGINE_ONLY.iter().map(|(n, _)| *n).collect();

    let missing: Vec<&String> = engine
        .keys()
        .filter(|n| !reference.contains_key(*n) && !excused.contains(n.as_str()))
        .collect();

    assert!(
        missing.is_empty(),
        "docs/spec/reference-types.rs bu tipleri BİLMİYOR: {missing:?}\n\
         Motorun modeli üst kümedir — ya referansa ekleyin ya da gerekçesiyle \
         `ENGINE_ONLY` listesine yazın.",
    );
}

#[test]
fn reference_model_declares_every_engine_field() {
    let engine = main_types(ENGINE_SRC);
    let reference = main_types(REFERENCE_SRC);
    let excused: BTreeSet<&str> = ENGINE_ONLY.iter().map(|(n, _)| *n).collect();

    let mut drift: Vec<String> = Vec::new();
    for (name, fields) in &engine {
        if excused.contains(name.as_str()) {
            continue;
        }
        let Some(ref_fields) = reference.get(name) else {
            continue; // ayrı test raporluyor
        };
        let missing: Vec<&String> = fields.difference(ref_fields).collect();
        if !missing.is_empty() {
            drift.push(format!("{name}: {missing:?}"));
        }
    }

    assert!(
        drift.is_empty(),
        "docs/spec/reference-types.rs bu alanları/varyantları TAŞIMIYOR:\n  {}\n\
         Motora alan eklendiğinde spec'teki referans model de güncellenir.",
        drift.join("\n  "),
    );
}

/// Referans model GERÇEK bir belgeyi okuyabilmeli — `deny_unknown_fields` taşıdığı için
/// bu, "spec modelinin kabul ettiği JSON motorun kabul ettiği JSON'dur" iddiasının
/// sınanmasıdır. Tek başına yeterli DEĞİLDİR (bkz. modül notu), ama ucuzdur.
#[test]
fn reference_model_parses_every_spec_example() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/spec/examples");
    let mut seen = 0usize;
    for entry in std::fs::read_dir(dir).expect("docs/spec/examples okunamadı") {
        let path = entry.expect("dizin girdisi").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("örnek okunamadı");
        let parsed: Result<reference::Wfd, _> = serde_json::from_str(&raw);
        assert!(
            parsed.is_ok(),
            "{} referans modelle parse EDİLEMEDİ: {}",
            path.display(),
            parsed.err().map(|e| e.to_string()).unwrap_or_default(),
        );
        seen += 1;
    }
    assert!(seen > 0, "hiç örnek belge bulunamadı — yol yanlış olabilir");
}
