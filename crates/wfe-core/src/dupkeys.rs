//! Çift JSON anahtarı dedektörü — BEŞ kök katalogda aynı id'nin İKİ KEZ geçmesini
//! yakalar: `nodes` · `actions` · `autoexec` · `calls` · `attachments`.
//!
//! Neden ayrı bir kapı: `serde_json` çift anahtarı HATA SAYMAZ, sessizce SONUNCUYU alır.
//! Yani `{"nodes":{"onay":{...A}, "onay":{...B}}}` belgesi hiçbir uyarı üretmeden A'yı
//! düşürür ve B ile çalışır. Node kimliği artık tasarımcının verdiği bir ad olduğundan
//! (2026-08-12) bu gerçek bir risk: iki farklı adım aynı kimliği alırsa biri sessizce
//! yok olur ve akış, tasarımcının çizdiğinden BAŞKA bir şey yapar.
//!
//! **v2.3 (`E10`) kapsamı BEŞE çıkardı.** v2.2'de yutulan şey bir etiketti; v2.3'te
//! yönlendirme kuralı aksiyon kaydının İÇİNDE olduğu için yutulan şey **bütün bir
//! yönlendirme kuralıdır** — belge doğrulamadan geçer, çalışır ve tasarımcının
//! çizdiğinden başka bir şey yapar. `attachments` de bir kök map: orada yutulan şey
//! bir belge grubu tanımıdır.
//!
//! `transitions` ve `terminals` **DİZİdir** — çift anahtar sorununa yapısal olarak
//! bağışık (ayrıştırıcı iki girdiyi de tutar, validator ikisini de görür); kapının
//! dışındadır. Aynı sebeple `attachments[].items` de buraya YAZILMAZ: item tekilliği
//! bir validator kuralıdır (`attachment_item_dup`), katman farkı bilinçlidir.
//!
//! Kapı ancak HAM METİN üzerinde kurulabilir: `Value`'ya dönüşmüş bir belgede
//! çakışma zaten silinmiştir. Bu yüzden `Wfd::from_value*` yolları bunu göremez —
//! çağrı, metne/bayta erişimi olan yerlerde yapılır (`Wfd::from_json*` + WFD gövdesi
//! taşıyan HER HTTP ucu, `Json<…>` yerine `Bytes` alarak).

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
    #[serde(default)]
    actions: Option<KeyList>,
    #[serde(default)]
    autoexec: Option<KeyList>,
    #[serde(default)]
    calls: Option<KeyList>,
    #[serde(default)]
    attachments: Option<KeyList>,
    /// **Zarf şekli.** Uçların çoğu belgeyi kökte değil bir istek gövdesinin `wfd`
    /// alanında taşır (`UploadBody`, `CreateDraftBody`, `SaveDraftBody`,
    /// `RunScenariosBody`, `SimStartBody`, `CreateTemplateBody`, …). Kapı yalnız köke
    /// bakarsa o gövdelerde HİÇBİR ŞEY görmez — zarfın kökünde `nodes` yoktur.
    #[serde(default)]
    wfd: Option<Box<Probe>>,
}

impl Probe {
    /// Yoklanan kataloglar, ADIYLA — hata mesajı hangi katalogda çakıştığını söyler.
    ///
    /// BEŞ kök map: `nodes` · `actions` · `autoexec` · `calls` · `attachments`.
    /// `transitions` ve `terminals` DİZİdir — yapısal olarak bağışık, kapının dışında.
    ///
    /// `wfd` alanı varsa oranın katalogları da yoklanır (`wfd.nodes` gibi adlandırılır).
    fn catalogs(&self) -> Vec<(String, &KeyList)> {
        let mut out: Vec<(String, &KeyList)> = [
            ("nodes", &self.nodes),
            ("actions", &self.actions),
            ("autoexec", &self.autoexec),
            ("calls", &self.calls),
            ("attachments", &self.attachments),
        ]
        .into_iter()
        .filter_map(|(name, slot)| slot.as_ref().map(|keys| (name.to_string(), keys)))
        .collect();
        if let Some(nested) = &self.wfd {
            out.extend(
                nested
                    .catalogs()
                    .into_iter()
                    .map(|(name, keys)| (format!("wfd.{name}"), keys)),
            );
        }
        out
    }
}

/// Ham WFD JSON'unda `nodes` katalogunun çift anahtar taşıyıp taşımadığını sorar.
///
/// Belge ayrıştırılamıyorsa `Ok(())` döner: burası ŞEKİL kapısı değil, yalnız çakışma
/// kapısıdır — bozuk JSON'un hatasını asıl ayrıştırıcı çok daha iyi anlatır ve aynı
/// hatayı iki kez raporlamak gerçek sebebi gölgeler.
pub fn assert_no_duplicate_catalog_ids(json: &[u8]) -> Result<(), EngineError> {
    let Ok(probe) = serde_json::from_slice::<Probe>(json) else {
        return Ok(());
    };
    // Tek geçişte HEPSİ toplanır: tasarımcı belgeyi bir kez düzeltsin, katalog başına
    // ayrı red turu yemesin.
    let mut findings: Vec<String> = Vec::new();
    for (catalog, KeyList(keys)) in probe.catalogs() {
        let mut seen = HashSet::new();
        let mut dups: Vec<&str> = keys
            .iter()
            .filter(|k| !seen.insert(k.as_str()))
            .map(String::as_str)
            .collect();
        if dups.is_empty() {
            continue;
        }
        dups.sort_unstable();
        dups.dedup();
        findings.push(format!("{catalog}: {}", dups.join(", ")));
    }
    if findings.is_empty() {
        return Ok(());
    }
    Err(EngineError::InvalidWfd(format!(
        "aynı id bir katalogda birden fazla kez tanımlı — {} — JSON'da çift anahtar \
         sessizce SONUNCUYU kazandırır, önceki tanım kaybolurdu",
        findings.join(" · ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_node_id_is_rejected() {
        let json = br#"{"nodes":{"onay":{"c_a":{}},"inceleme":{"c_a":{}},"onay":{"c_a":{}}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("çift anahtar reddedilmeli");
        assert!(format!("{err:?}").contains("onay"), "hata id'yi söylemeli: {err:?}");
    }

    #[test]
    fn distinct_node_ids_pass() {
        let json = br#"{"nodes":{"onay":{"c_a":{}},"inceleme":{"c_a":{}}}}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn missing_or_unparsable_input_is_not_our_error() {
        assert!(assert_no_duplicate_catalog_ids(br#"{"id":"x"}"#).is_ok());
        assert!(assert_no_duplicate_catalog_ids(b"{bozuk").is_ok());
    }

    #[test]
    fn duplicate_action_id_is_rejected() {
        // v2.3: yönlendirme kuralı aksiyon kaydının İÇİNDE. Yutulan şey bir etiket
        // değil, bütün bir yönlendirme kuralı olurdu.
        let json = br#"{"actions":{"onayla":{"input":{}},"reddet":{"input":{}},"onayla":{"input":{}}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("çift anahtar reddedilmeli");
        let msg = format!("{err:?}");
        assert!(msg.contains("onayla"), "hata id'yi söylemeli: {msg}");
        assert!(msg.contains("actions"), "hata KATALOGU söylemeli: {msg}");
    }

    #[test]
    fn distinct_action_ids_pass() {
        let json = br#"{"actions":{"onayla":{"input":{}},"reddet":{"input":{}}}}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn duplicate_autoexec_id_is_rejected() {
        let json = br#"{"autoexec":{"skor":{"kind":"http"},"kur":{"kind":"http"},"skor":{"kind":"http"}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("çift anahtar reddedilmeli");
        let msg = format!("{err:?}");
        assert!(msg.contains("skor") && msg.contains("autoexec"), "{msg}");
    }

    #[test]
    fn distinct_autoexec_ids_pass() {
        let json = br#"{"autoexec":{"skor":{"kind":"http"},"kur":{"kind":"http"}}}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn duplicate_call_id_is_rejected() {
        let json = br#"{"calls":{"skorla":{"wfd_id":"a"},"kullandir":{"wfd_id":"b"},"skorla":{"wfd_id":"c"}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("çift anahtar reddedilmeli");
        let msg = format!("{err:?}");
        assert!(msg.contains("skorla") && msg.contains("calls"), "{msg}");
    }

    #[test]
    fn distinct_call_ids_pass() {
        let json = br#"{"calls":{"skorla":{"wfd_id":"a"},"kullandir":{"wfd_id":"b"}}}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn duplicate_attachment_group_id_is_rejected() {
        // E10: `attachments` de bir kök map — yutulan şey bir BELGE GRUBU tanımı olurdu.
        let json = br#"{"attachments":{"kimlik":{"items":[]},"gelir":{"items":[]},"kimlik":{"items":[]}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("çift anahtar reddedilmeli");
        let msg = format!("{err:?}");
        assert!(msg.contains("kimlik") && msg.contains("attachments"), "{msg}");
    }

    #[test]
    fn distinct_attachment_group_ids_pass() {
        let json = br#"{"attachments":{"kimlik":{"items":[]},"gelir":{"items":[]}}}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn document_nested_under_a_request_envelope_is_gated() {
        // `POST /wfd`in gövdesi `UploadBody { orgtnt_id, project_id?, wfd }` — belge
        // KÖKTE değil, `wfd` alanının altında. Kapı zarfın kökünü yoklarsa hiçbir şey
        // görmez: zarfın kökünde `nodes` YOKTUR. Aynı şekil 11 uçta daha var
        // (`create_draft`, `save_draft`, `sim_*`, `create_template`, …).
        let json = br#"{"orgtnt_id":"a","wfd":{"nodes":{"onay":{},"onay":{}}}}"#;
        let err = assert_no_duplicate_catalog_ids(json)
            .expect_err("zarfın içindeki belge de kapıdan geçmeli");
        assert!(format!("{err:?}").contains("onay"), "{err:?}");
    }

    #[test]
    fn duplicates_in_several_catalogs_are_all_reported_in_one_call() {
        // Kapı tek geçişte HEPSİNİ söyler: tasarımcı belgeyi bir kez düzeltsin, üç
        // ayrı red turu yemesin.
        let json = br#"{"nodes":{"onay":{},"onay":{}},"actions":{"gonder":{},"gonder":{}},"calls":{"skor":{},"skor":{}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("reddedilmeli");
        let msg = format!("{err:?}");
        for expected in ["nodes", "onay", "actions", "gonder", "calls", "skor"] {
            assert!(msg.contains(expected), "'{expected}' mesajda yok: {msg}");
        }
    }

    #[test]
    fn arrays_are_out_of_the_gate() {
        // `transitions`/`terminals`/`start` DİZİdir — çift anahtar sorunu yapısal olarak
        // yaşanmaz; ayrıştırıcı iki girdiyi de tutar. Kapı onlara bakmaz.
        let json = br#"{"transitions":[{"id":"t1"},{"id":"t1"}],"terminals":[{"id":"x"},{"id":"x"}]}"#;
        assert!(assert_no_duplicate_catalog_ids(json).is_ok());
    }

    #[test]
    fn duplicates_are_reported_once_even_if_repeated_many_times() {
        let json = br#"{"nodes":{"a":{},"a":{},"a":{},"b":{},"b":{}}}"#;
        let err = assert_no_duplicate_catalog_ids(json).expect_err("reddedilmeli");
        let msg = format!("{err:?}");
        assert_eq!(msg.matches("\"a\"").count() + msg.matches(" a,").count(), 1, "{msg}");
    }
}
