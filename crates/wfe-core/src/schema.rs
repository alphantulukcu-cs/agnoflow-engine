//! Kanonik JSON Schema kapısı — `docs/spec/schema.json` motorun İÇİNE gömülüdür.
//!
//! Neden: `wfd_version` kapısı + serde + `validator` üçlüsü şemayı TAM karşılamıyordu.
//! serde `#[serde(default)]`li her alanı eksik kabul eder, `minItems`/`uniqueItems`/
//! `pattern` gibi kısıtları hiç bilmez; `validator` ise elle yazılmış kural setidir.
//! Sonuç: editörden geçmeyen (elle yazılıp API'ye POST edilen) bir doküman şemayı ihlal
//! ettiği halde kabul ediliyordu — ör. `"c_r": []` şemada `minItems: 1` ile yasak, serde
//! için `Some([])`, motor için "rol kanalı kapalı". Belge geçersizdi ama çalışıyordu.
//!
//! Kapı ARTIK motorda: şema kanonik dosyadan `include_str!` ile derleme anında gömülür,
//! yani binary ile spec ayrı düşemez. Editör (`agnoflow-frontend/src/schema/wfd.schema.json`)
//! aynı dosyanın kopyasını ajv ile koşar — kural seti motorun, editör yalnız aynı cevabı
//! önden verir (ifade doğrulamasındaki `validate-expression` sözleşmesinin aynısı).
//!
//! Kapı NEREDE koşar: WFD yazma yollarının hepsi (upload / publish / submit / approve) VE
//! okuma (fetch) — `wfd::adapter`. Taslak KAYDI (`save_draft`) kapsam DIŞIDIR: yarım
//! belge kaydedilebilir, yayınlanamaz.

use serde_json::Value;
use std::sync::OnceLock;

/// Kanonik şema — `docs/spec/schema.json`. Derleme anında gömülür (tek kaynak).
pub const SCHEMA_JSON: &str = include_str!("../../../docs/spec/schema.json");

fn validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| {
        let schema: Value = serde_json::from_str(SCHEMA_JSON)
            .expect("docs/spec/schema.json parse edilemedi (gömülü kanonik şema)");
        jsonschema::validator_for(&schema)
            .expect("docs/spec/schema.json geçerli bir JSON Schema değil")
    })
}

/// Bir WFD dokümanını kanonik şemaya karşı doğrular.
///
/// Hata mesajları `<yol>: <ihlal>` biçiminde ve deterministik sıradadır (ajv tarafındaki
/// `formatFriendlyAjvErrors` ile aynı amaç: JSON-path taşıyan, tek satırlık ihlal).
/// En çok `MAX_ERRORS` ihlal döner — geri kalan sayı olarak eklenir; 300 node'lu bir
/// belgede tek bir yapısal hata yüzlerce ihlal üretebiliyor.
pub fn validate_document(doc: &Value) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut extra = 0usize;
    for err in validator().iter_errors(doc) {
        if errors.len() < MAX_ERRORS {
            let path = err.instance_path.to_string();
            let path = if path.is_empty() { "(kök)".into() } else { path };
            errors.push(format!("{path}: {err}"));
        } else {
            extra += 1;
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    errors.sort();
    errors.dedup();
    if extra > 0 {
        errors.push(format!("... ve {extra} ihlal daha"));
    }
    Err(errors)
}

const MAX_ERRORS: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Gömülü şema DERLENEBİLİR olmalı — bozuk `$ref`/geçersiz keyword derleme
    /// anında değil ilk kullanımda patlar, o yüzden testle sabitlenir.
    #[test]
    fn embedded_schema_compiles() {
        let _ = validator();
    }

    #[test]
    fn golden_fixture_conforms() {
        let text = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");
        let doc: Value = serde_json::from_str(text).unwrap();
        assert_eq!(validate_document(&doc), Ok(()));
    }

    /// Elle yazılan JSON'un kaçak yolu: serde `[]`'i kabul ediyor, şema etmiyor.
    #[test]
    fn empty_c_r_is_rejected() {
        let doc = minimal_doc(json!({ "c_orgu": "self", "c_r": [], "c_u": ["ayse"] }));
        let errs = validate_document(&doc).unwrap_err();
        assert!(
            errs.iter().any(|e| e.contains("c_r")),
            "boş c_r reddedilmeli: {errs:?}"
        );
    }

    /// Çapasız biçim: yalnız `c_u`.
    #[test]
    fn anchorless_c_u_only_is_accepted() {
        let doc = minimal_doc(json!({ "c_u": ["ayse"] }));
        assert_eq!(validate_document(&doc), Ok(()));
    }

    /// Çapasız + rol = en geniş kapı; şema reddeder.
    #[test]
    fn anchorless_with_c_r_is_rejected() {
        let doc = minimal_doc(json!({ "c_r": ["mudur"] }));
        assert!(validate_document(&doc).is_err());
        let doc = minimal_doc(json!({ "c_u": ["ayse"], "c_r": ["mudur"] }));
        assert!(validate_document(&doc).is_err());
    }

    #[test]
    fn empty_rule_is_rejected() {
        let doc = minimal_doc(json!({}));
        assert!(validate_document(&doc).is_err());
    }

    /// Golden fixture'ın İLK node'unun `c_a`'sı değiştirilmiş kopyası — şema kuralı
    /// tek başına sınanır, dokümanın kalanı zaten uyumlu. (Elle yazılan minimal belge
    /// şemanın 10 zorunlu kök alanını taşımak zorunda; sınamak istediğimiz o değil.)
    fn minimal_doc(c_a: Value) -> Value {
        let text = include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json");
        let mut doc: Value = serde_json::from_str(text).unwrap();
        let nodes = doc["nodes"].as_object_mut().unwrap();
        let first = nodes.keys().next().unwrap().clone();
        nodes[&first]["c_a"] = c_a;
        doc
    }
}

#[cfg(test)]
mod fixture_audit {
    //! Depodaki TÜM örnek/fixture belgeleri şema kapısından geçmek ZORUNDA — okuma yolu da
    //! doğrulandığı için (2026-08-07 kararı) şemaya uymayan bir belge çalıştırılamaz.
    use super::*;

    fn check(name: &str, text: &str) {
        let doc: serde_json::Value = serde_json::from_str(text).expect(name);
        assert_eq!(validate_document(&doc), Ok(()), "{name} şemayı ihlal ediyor");
    }

    #[test]
    fn spec_examples_conform() {
        check("kredi-basvuru.golden", include_str!("../../../docs/spec/examples/kredi-basvuru.golden.json"));
        check("akis-cagrisi", include_str!("../../../docs/spec/examples/akis-cagrisi.json"));
        check("belge-onay", include_str!("../../../docs/spec/examples/belge-onay.json"));
        check("kredi-kullandirim", include_str!("../../../docs/spec/examples/kredi-kullandirim.json"));
        check("kredi-skor", include_str!("../../../docs/spec/examples/kredi-skor.json"));
        check("paralel-onay", include_str!("../../../docs/spec/examples/paralel-onay.json"));
    }

    #[test]
    fn test_fixtures_conform() {
        check("fixtures/kredi-basvuru.golden", include_str!("../tests/fixtures/kredi-basvuru.golden.json"));
        check("fixtures/paralel-onay", include_str!("../tests/fixtures/paralel-onay.json"));
        check("fixtures/belge-onay", include_str!("../tests/fixtures/belge-onay.json"));
    }
}
