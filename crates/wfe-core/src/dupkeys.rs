//! Çift JSON anahtarı dedektörü — `nodes` katalogunda aynı id'nin İKİ KEZ geçmesini
//! yakalar.
//!
//! Neden ayrı bir kapı: `serde_json` çift anahtarı HATA SAYMAZ, sessizce SONUNCUYU alır.
//! Yani `{"nodes":{"onay":{...A}, "onay":{...B}}}` belgesi hiçbir uyarı üretmeden A'yı
//! düşürür ve B ile çalışır. Node kimliği artık tasarımcının verdiği bir ad olduğundan
//! (2026-08-12) bu gerçek bir risk: iki farklı adım aynı kimliği alırsa biri sessizce
//! yok olur ve akış, tasarımcının çizdiğinden BAŞKA bir şey yapar.
//!
//! Kapı ancak HAM METİN üzerinde kurulabilir: `Value`'ya dönüşmüş bir belgede
//! çakışma zaten silinmiştir. Bu yüzden `Wfd::from_value*` yolları bunu göremez —
//! çağrı, metne/bayta erişimi olan yerlerde yapılır (`Wfd::from_json*`, upload rotası).

use crate::error::EngineError;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use std::collections::HashSet;
use std::fmt;

/// Bir JSON objesinin anahtarlarını GELİŞ SIRASIYLA toplar — `Map` gibi tekilleştirmez.
struct KeyList(Vec<String>);

impl<'de> serde::Deserialize<'de> for KeyList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Vec<String>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("bir JSON objesi")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Vec<String>, A::Error> {
                let mut keys = Vec::new();
                while let Some((k, _)) = m.next_entry::<String, IgnoredAny>()? {
                    keys.push(k);
                }
                Ok(keys)
            }
        }
        d.deserialize_map(V).map(KeyList)
    }
}

#[derive(serde::Deserialize)]
struct Probe {
    #[serde(default)]
    nodes: Option<KeyList>,
}

/// Ham WFD JSON'unda `nodes` katalogunun çift anahtar taşıyıp taşımadığını sorar.
///
/// Belge ayrıştırılamıyorsa `Ok(())` döner: burası ŞEKİL kapısı değil, yalnız çakışma
/// kapısıdır — bozuk JSON'un hatasını asıl ayrıştırıcı çok daha iyi anlatır ve aynı
/// hatayı iki kez raporlamak gerçek sebebi gölgeler.
pub fn assert_no_duplicate_node_ids(json: &[u8]) -> Result<(), EngineError> {
    let Ok(probe) = serde_json::from_slice::<Probe>(json) else {
        return Ok(());
    };
    let Some(KeyList(keys)) = probe.nodes else {
        return Ok(());
    };
    let mut seen = HashSet::new();
    let mut dups: Vec<String> = keys
        .into_iter()
        .filter(|k| !seen.insert(k.clone()))
        .collect();
    if dups.is_empty() {
        return Ok(());
    }
    dups.sort();
    dups.dedup();
    Err(EngineError::InvalidWfd(format!(
        "aynı node id'si birden fazla kez tanımlı: {} — JSON'da çift anahtar sessizce \
         SONUNCUYU kazandırır, önceki tanım kaybolurdu",
        dups.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_node_id_is_rejected() {
        let json = br#"{"nodes":{"onay":{"c_a":{}},"inceleme":{"c_a":{}},"onay":{"c_a":{}}}}"#;
        let err = assert_no_duplicate_node_ids(json).expect_err("çift anahtar reddedilmeli");
        assert!(format!("{err:?}").contains("onay"), "hata id'yi söylemeli: {err:?}");
    }

    #[test]
    fn distinct_node_ids_pass() {
        let json = br#"{"nodes":{"onay":{"c_a":{}},"inceleme":{"c_a":{}}}}"#;
        assert!(assert_no_duplicate_node_ids(json).is_ok());
    }

    #[test]
    fn missing_or_unparsable_input_is_not_our_error() {
        assert!(assert_no_duplicate_node_ids(br#"{"id":"x"}"#).is_ok());
        assert!(assert_no_duplicate_node_ids(b"{bozuk").is_ok());
    }

    #[test]
    fn duplicates_are_reported_once_even_if_repeated_many_times() {
        let json = br#"{"nodes":{"a":{},"a":{},"a":{},"b":{},"b":{}}}"#;
        let err = assert_no_duplicate_node_ids(json).expect_err("reddedilmeli");
        let msg = format!("{err:?}");
        assert_eq!(msg.matches("\"a\"").count() + msg.matches(" a,").count(), 1, "{msg}");
    }
}
