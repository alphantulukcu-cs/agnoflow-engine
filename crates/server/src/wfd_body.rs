//! WFD dokümanı taşıyan istek gövdelerinin ORTAK giriş kapısı.
//!
//! Neden ayrı bir yardımcı: `Json<T>` ekstraktörü fonksiyon gövdesi başlamadan
//! `serde_json`u çalıştırır ve çift JSON anahtarı **ORADA silinir** — sessizce
//! sonuncusu kazanır. Ekstraktörden sonra kurulan hiçbir kontrol çakışmayı göremez,
//! çünkü bakacağı veri artık yoktur. Bu yüzden WFD gövdesi taşıyan her uç
//! `axum::body::Bytes` alır ve İLK İŞ olarak buradan geçer.
//!
//! v2.2'de yutulan şey bir etiketti; v2.3'te yönlendirme kuralı aksiyon kaydının
//! İÇİNDE olduğu için yutulan şey **bütün bir yönlendirme kuralıdır**: belge
//! doğrulamadan geçer, çalışır ve tasarımcının çizdiğinden başka bir şey yapar.

use crate::error::AppError;
use axum::http::StatusCode;
use serde::de::DeserializeOwned;

/// Ham istek baytını çift-anahtar kapısından geçirir, sonra gövdeye ayrıştırır.
///
/// Sıra bilinçli: kapı ÖNCE. Bozuk JSON'da kapı `Ok(())` döner (şekil kapısı değil),
/// dolayısıyla asıl ayrıştırma hatası gölgelenmez.
pub fn parse_wfd_body<T: DeserializeOwned>(raw: &[u8]) -> Result<T, AppError> {
    wfe_core::dupkeys::assert_no_duplicate_catalog_ids(raw).map_err(AppError::from)?;
    serde_json::from_slice(raw)
        .map_err(|e| AppError(format!("gövde ayrıştırılamadı: {e}"), StatusCode::BAD_REQUEST))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::Value;

    #[derive(Deserialize, Debug)]
    struct Envelope {
        wfd: Value,
    }

    #[test]
    fn duplicate_catalog_key_in_the_nested_document_is_rejected() {
        let raw = br#"{"wfd":{"nodes":{"onay":{},"onay":{}}}}"#;
        let err = parse_wfd_body::<Envelope>(raw).expect_err("çift anahtar reddedilmeli");
        assert_eq!(err.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", err.message);
        assert!(err.message.contains("onay"), "hata id'yi söylemeli: {}", err.message);
    }

    #[test]
    fn duplicate_catalog_key_at_the_root_is_rejected() {
        // `validate_wfd` gibi gövdenin KENDİSİ belge olan uçlar.
        let raw = br#"{"nodes":{"onay":{},"onay":{}},"actions":{}}"#;
        let err = parse_wfd_body::<Value>(raw).expect_err("çift anahtar reddedilmeli");
        assert!(err.message.contains("onay"), "{}", err.message);
    }

    #[test]
    fn clean_body_parses() {
        let raw = br#"{"wfd":{"nodes":{"onay":{},"inceleme":{}}}}"#;
        let body = parse_wfd_body::<Envelope>(raw).expect("temiz gövde geçmeli");
        assert!(body.wfd["nodes"]["inceleme"].is_object());
    }

    #[test]
    fn malformed_json_reports_the_parse_error_not_the_gate() {
        let err = parse_wfd_body::<Envelope>(b"{bozuk").expect_err("bozuk JSON reddedilmeli");
        assert_eq!(err.status, StatusCode::BAD_REQUEST, "{}", err.message);
        assert!(err.message.contains("ayrıştırılamadı"), "{}", err.message);
    }
}
