//! `$`-önekli değer referanslarının GRAMERİ — tek kaynak.
//!
//! # Neden ayrı modül
//!
//! Motor bir effect/config/input değerini çözerken tanımadığı `$`-string'i HATA saymaz,
//! **düz metin sabiti** olarak yazar (`effects::resolve_dollar_string` son satırı:
//! `Ok(Value::from(s))`). Bu, `$actor.role` ya da `$call.state` gibi bir yazım hatasının
//! yayında hiçbir iz bırakmaması demektir: alana `"$actor.role"` METNİ yazılır, o alanı
//! okuyan koşullar sessizce hep-false olur, log da çıkmaz.
//!
//! Çözüm referansı çözmeye çalışmak DEĞİL (motor davranışı yerinde: `$` ile başlayan her
//! metni referans saysaydık `"$100 ödendi"` gibi meşru sabitler patlardı), tasarım
//! zamanında REDDETMEKtir. Bunun için "motorun tanıdığı biçimler" listesinin tek bir
//! yerde durması gerekir — çözücüler dört ayrı yerde:
//!
//! * `v22::effects::resolve_dollar_string` — `wfes_effects.set`
//! * `v22::pipeline` — WFC `call.input` ve terminal `wfe_end_response` (aynı `resolve_value`)
//! * `wfe::runner::resolve_config_string` — autoexec `config` (ek olarak SECRET `$env`)
//!
//! Yeni bir namespace eklendiğinde buradaki tablo da genişletilmelidir; genişletilmezse
//! validator o namespace'i "tanınmayan referans" diye reddeder — sessiz sapma yerine
//! gürültülü hata, istenen budur.

/// Bir string değerinin `$` grameri karşısındaki konumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DollarForm {
    /// Motorun çözdüğü bir referans (ya da `$env` — kendi kuralı ayrıca denetler).
    Known,
    /// Referans DEĞİL, düz metin sabiti: `"$100"`, `"toplam $"`, `"$ tutar"`.
    /// Motor da öyle yazar; kural karışmaz.
    PlainText,
    /// Referans GİBİ yazılmış ama tabloda yok: `$actor.role`, `$call.state`, `$ctxx.a`.
    /// Motor bunu düz metin yazar → tasarım hatası.
    Unknown,
}

/// Tam eşleşen referanslar (motorun `match` kolları).
const EXACT: &[&str] = &[
    "$actor",
    "$timestamp",
    "$wfe_id",
    "$node",
    "$call.status",
    "$call.wfe_id",
];

/// Yol taşıyan referanslar — önek + BOŞ OLMAYAN yol.
const PREFIXES: &[&str] = &[
    "$ctx.",
    "$action.input.",
    "$exec.result.",
    "$call.result.",
    // M7'de kaldırıldı ama kendi kuralı (`exec_response`) daha iyi bir mesaj veriyor;
    // burada "tanınmayan" diye ikinci bir hata üretmeyelim.
    "$exec.response.",
];

/// Referans GİBİ mi yazılmış: `$` + tanımlayıcı, aralarında nokta.
///
/// `"$100"` (rakamla başlar) ve `"toplam $"` bu kalıba uymaz — motor onları metin yazar,
/// tasarımcı da öyle istemiştir. Kural yalnız gerçekten referans denemesi olan
/// string'lere uygulanır ki meşru sabitler yayından düşmesin.
fn looks_like_ref(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('$') else {
        return false;
    };
    if rest.is_empty() {
        return false;
    }
    let mut prev_dot = true; // baştaki segment de harfle başlamalı
    for c in rest.chars() {
        if c == '.' {
            if prev_dot {
                return false; // `$..a` / `$.a` — referans denemesi sayılmaz
            }
            prev_dot = true;
            continue;
        }
        if prev_dot && !(c.is_ascii_alphabetic() || c == '_') {
            return false; // segment harf ya da `_` ile başlar (`$100` elenir)
        }
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        prev_dot = false;
    }
    // Sonda nokta KALABİLİR (`$call.`): yarım yazılmış bir referans da referans denemesidir,
    // motor onu düz metin yazacağı için sessizce geçmemeli.
    true
}

/// Bir değer string'inin grameri (bkz. `DollarForm`).
pub fn classify(s: &str) -> DollarForm {
    // `$env` ARA-DEĞER de çözülür (`env::resolve_string`), yani string'in tamamı olmak
    // zorunda değil: `"$env.AUTH_API/v1/users"` meşrudur. Biçim denetimi env'in kendi
    // kuralına (`env_reference_malformed`) aittir.
    if crate::v22::env::contains_ref(s) {
        return DollarForm::Known;
    }
    if EXACT.contains(&s) {
        return DollarForm::Known;
    }
    for p in PREFIXES {
        if let Some(path) = s.strip_prefix(p) {
            // Boş yol (`$ctx.`) referans değil, yazım hatasıdır.
            return if path.is_empty() {
                DollarForm::Unknown
            } else {
                DollarForm::Known
            };
        }
    }
    if looks_like_ref(s) {
        DollarForm::Unknown
    } else {
        DollarForm::PlainText
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_forms_are_accepted() {
        for s in [
            "$actor",
            "$timestamp",
            "$wfe_id",
            "$node",
            "$call.status",
            "$call.wfe_id",
            "$ctx.tutar",
            "$ctx.musteri.ad",
            "$action.input.tutar",
            "$exec.result.skor",
            "$call.result.karar",
            "$env.AUTH_API",
            "$env.AUTH_API/v1/users",
            "prefix $env.REGION suffix",
        ] {
            assert_eq!(classify(s), DollarForm::Known, "{s}");
        }
    }

    /// Motorun tablosunda OLMAYAN ama referans gibi yazılmış her şey.
    #[test]
    fn near_miss_references_are_unknown() {
        for s in [
            "$actor.role",      // çıplak `$actor` var, alt yolu YOK
            "$call.state",      // doğrusu `$call.status`
            "$ctxx.tutar",      // yazım hatası
            "$action.inputs.x", // yazım hatası
            "$prev.actor.role", // ZEN namespace'i, effects'te yok
            "$wfah",
            "$ctx.",  // boş yol
            "$call.", // boş yol
        ] {
            assert_eq!(classify(s), DollarForm::Unknown, "{s}");
        }
    }

    /// Meşru metin sabitleri kurala takılmamalı — `$` her yerde referans değildir.
    #[test]
    fn plain_text_is_not_a_reference() {
        for s in [
            "$100",
            "$",
            "toplam $",
            "fiyat: 10$",
            "$ tutar",
            "$1.5",
            "onaylandı",
        ] {
            assert_eq!(classify(s), DollarForm::PlainText, "{s}");
        }
    }
}
