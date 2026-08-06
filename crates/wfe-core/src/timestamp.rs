//! Zaman damgası METNİNİN tek kaynağı: UTC `yyyyMMddHHmmss`, 14 rakam.
//!
//! # Neden RFC3339 değil
//!
//! `$wfah` girdisinin `at` alanı ve `$timestamp` namespace'i ZEN ifadelerinde
//! karşılaştırılır ve ZEN'in karşılaştırması bu iki biçimde çok farklı davranır:
//!
//! * `Opcode::Compare` (`> < >= <=`) String/String çiftinde **runtime hatası** verir
//!   (`Compare: Unsupported type`) — RFC3339 da metin olduğu için sıralama ancak `d()`
//!   sarmalıyla çalışırdı, yani tasarımcı her tarih koşulunda serbest ZEN yazmak
//!   zorunda kalıyordu.
//! * RFC3339 SABİT UZUNLUKLU DEĞİLDİR ve saat dilimi eki taşır
//!   (`2026-01-15T10:30:00+03:00` · `…Z` · kesirli saniye) → `startsWith` ile "o gün"
//!   sorgusu bile ayırıcı karakterlere bağımlı, `==` ise pratikte hiç tutmuyor.
//!
//! `yyyyMMddHHmmss` ikisini de çözer: sabit 14 hane, yalnız rakam, ayırıcı yok ve
//! **leksikografik sıra = kronolojik sıra** — yani `startsWith` ile yıl/ay/gün/saat
//! öneki doğal olarak çalışır, sabit karşılaştırmalar da string temellidir.
//!
//! Editör tarafındaki karşılığı `zenFunctions.WFAH_TIMESTAMP_FIELDS` +
//! `whenFields.isTimestampLiteral`; motorun kapısı `expr_types::is_timestamp_literal`.
//! Üç yer de AYNI biçimi tanımak zorundadır.

use chrono::{DateTime, Utc};

/// `yyyyMMddHHmmss` biçim dizgisi — `chrono::format` için.
pub const TIMESTAMP_FORMAT: &str = "%Y%m%d%H%M%S";

/// Uzunluk sözleşmesi: 14 karakter (`yyyy`4 + `MM`2 + `dd`2 + `HH`2 + `mm`2 + `ss`2).
pub const TIMESTAMP_LEN: usize = 14;

/// Bir UTC zamanının ZEN'e/context'e yazılan metni.
pub fn timestamp_string(t: DateTime<Utc>) -> String {
    t.format(TIMESTAMP_FORMAT).to_string()
}

/// Şu anın damgası — `$timestamp` bunu üretir.
pub fn now_timestamp() -> String {
    timestamp_string(Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn formats_fourteen_digits_utc() {
        let t = Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap();
        assert_eq!(timestamp_string(t), "20260115103000");
        assert_eq!(timestamp_string(t).len(), TIMESTAMP_LEN);
        assert!(timestamp_string(t).bytes().all(|c| c.is_ascii_digit()));
    }

    /// Sözleşmenin ASIL faydası: metin sırası = zaman sırası. Editör bu yüzden
    /// `startsWith` önekleriyle "o gün/ay/yıl" sorgusunu kurabiliyor.
    #[test]
    fn lexicographic_order_matches_chronological_order() {
        let a = timestamp_string(Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap());
        let b = timestamp_string(Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 1).unwrap());
        let c = timestamp_string(Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap());
        assert!(a < b && b < c);
        assert!(c.starts_with("202602"));
    }
}
