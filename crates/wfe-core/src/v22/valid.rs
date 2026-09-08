//! `$valid` — ELENMİŞ geçmiş (WFD v2.3, K7–K9, §3.1).
//!
//! Ham `$wfah` bir DEFTERDİR: iptal edilmiş kolun onayı da, geri gönderme öncesi
//! turun onayı da orada durur. `count($wfah, …) >= 2` bu yüzden yanlış cevap
//! üretiyor. `$valid` aynı satırları elemeden geçirir; **saklanmaz**, her okumada
//! defterden türetilir (§3.1 "Saflık" — tek girdi defterdir).
//!
//! **BU MODÜL ŞU AN YALNIZ ELEME KURALI 5'İ (TUR) TAŞIR.** Kalan dört kural ve
//! `$valid`in ZEN yüzeyi (`Root::ValidEntry`, `ValidRules::for_version` dikişi,
//! hesaplanan `first_by_*` alanları) `E05`in işidir; `E14` onun ÖN ŞARTIDIR ve
//! önce iniyor. Sıra bağlayıcıdır: belirsiz bir kol kimliği sözleşmeye girmesin.
//!
//! ## Eleme kuralı 5 — TUR (`E14`/S1)
//!
//! Aynı fork'a İKİNCİ kez girilebilir (join sonrası bir döngüyle, ya da geri
//! gönderme sonrası). İkinci tur aynı `entry_node`'u yeniden kullanır — yani
//! birinci turun satırları ile ikinci turunkiler AYNI `branch_entry`'yi taşır.
//! Ayrım yapılmazsa iki tur toplanır ve ikinci turun eşiği birinci turun
//! onaylarıyla dolar.
//!
//! Kural: `branch_entry` = X taşıyan bir satır, X'i açan **SON `_fork` satırının
//! `seq`'inden ÖNCE** ise elenir. `branch_entry` NULL olan satırlara DOKUNULMAZ.
//!
//! Çapa neden `_fork`: o satır her fork commit'inde KOŞULSUZ yazılıyor
//! (`pipeline::stage_parallel_markers`), yani tur sınırı deftere ZATEN düşüyor —
//! eleme tamamen OKUMA tarafındadır, fork commit'ine yeni yazma EKLENMEZ. Kol
//! tablosuna (`wf.wfe_branch`) bakılamaz: o tablo yalnız YAŞAYAN turu taşır
//! (`E14`/S2), kapanan turun satırları silinir.
//!
//! `E05`/BAĞLI KURAL 3 (fork giriş node'u belge genelinde tekil) sayesinde X tek
//! bir fork'a aittir → çapa TEKTİR. Kural `Ç4` ile aynı yöndedir (`seq` yönlü).

use crate::types::wfah::{Wfah, WfahEntry};
use serde_json::Value;

/// Fork marker'ı — kolları YARATAN satır. Adı sözleşmedir (Değişmez #2).
const FORK: &str = "_fork";
/// Paralel modu KAPATAN marker'lar. `_join` AND/quorum join'de (adapter + sim),
/// `_collapse` collapse / terminal / failed / terminated / quorum yollarında yazılır
/// (`pipeline::stage_parallel_markers`). İkisi birlikte paralel modu bitiren TÜM
/// yolları kapsar.
const CLOSERS: [&str; 2] = ["_join", "_collapse"];

/// Bir `_fork` satırının açtığı kolların giriş node'ları (`input.branches`).
///
/// Bu okuma `_fork` payload'ının şeklini bir SÖZLEŞMEYE çevirir (`E14`, FEDA
/// EDİLENLER): `branches` bugüne kadar yalnız audit kaydıydı, artık tur türetiminin
/// ÇAPASIDIR ve sessizce değiştirilemez.
fn fork_branches(entry: &WfahEntry) -> &[Value] {
    entry
        .input
        .as_ref()
        .and_then(|i| i.get("branches"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// `_fork` satırı verilen giriş node'unu açmış mı.
pub fn fork_opens(entry: &WfahEntry, branch_entry: &str) -> bool {
    fork_branches(entry)
        .iter()
        .any(|b| b.as_str() == Some(branch_entry))
}

/// Verilen kolu açan `_fork` satırlarının SAYISI = kolun o anki turu.
/// **1'den başlar ve FORK BAŞINA sayar** — global bir sayaç DEĞİL.
pub fn round_of(wfah: &Wfah, branch_entry: &str) -> u32 {
    wfah.entries()
        .iter()
        .filter(|e| e.action == FORK && fork_opens(e, branch_entry))
        .count() as u32
}

/// `round_of`un `Option` sarmalı — kol etiketi olmayan satır için `None`.
/// `branch_entry` NULL ⇔ `branch_round` NULL değişmezini (E14/S3) çağıranın
/// unutmasına yer bırakmaz. Taban 1'e sabitlenir: kol bağlamında `_fork` satırı
/// olmadan satır üretilemez, ama 0 dönmek *"bu satır bir kolda değil"* anlamına
/// gelirdi.
pub fn round_of_opt(wfah: &Wfah, branch_entry: Option<&str>) -> Option<u32> {
    branch_entry.map(|e| round_of(wfah, e).max(1))
}

/// Verilen kolu açan SON `_fork` satırının `seq`'i — tur sınırı.
fn last_fork_seq(wfah: &Wfah, branch_entry: &str) -> Option<u32> {
    wfah.entries()
        .iter()
        .rev()
        .find(|e| e.action == FORK && fork_opens(e, branch_entry))
        .map(|e| e.seq)
}

/// **Eleme kuralı 5 (TUR).** Satır, kolunun YAŞAYAN turundan önce mi yazıldı?
///
/// `branch_entry` NULL olan satırlar (fork öncesi/sonrası satırlar, `_fork`/`_join`/
/// `_collapse` marker'ları, paralel olmayan WFE'lerin tüm satırları) HİÇ elenmez.
pub fn eliminated_by_round(wfah: &Wfah, row: &WfahEntry) -> bool {
    let Some(entry) = row.branch_entry.as_deref() else {
        return false;
    };
    match last_fork_seq(wfah, entry) {
        Some(boundary) => row.seq < boundary,
        // Kolu açan `_fork` satırı yoksa elenecek tur sınırı da yoktur (R01: eski
        // şekilli satır için okuyucu YAZILMAZ, veri sıfırlanır).
        None => false,
    }
}

/// `$branch_round` kökünün değeri — YAŞAYAN turun numarası, paralel mod dışında
/// `None` (ZEN'de `null`).
///
/// Tek girdisi defterdir: son `_fork` satırından SONRA bir kapatıcı marker
/// (`_join`/`_collapse`) varsa paralel mod kapanmıştır. İç içe fork YASAK olduğu
/// için (`validator.rs`, `parallel_nested`) paralel modda tek bir fork yaşar ve o
/// fork'un bütün kolları AYNI `_fork` satırlarından doğar — tur bütün kollarda
/// aynıdır, kol seçmek gerekmez.
///
/// Değerlendirme, satırları yazan commit'ten ÖNCEki defteri görür; yani paralel modu
/// KAPATAN commit'in kendi ifadeleri hâlâ o turun içinde okur (doğru olan bu).
pub fn live_round(wfah: &Wfah) -> Option<u32> {
    let last_fork = wfah.entries().iter().rev().find(|e| e.action == FORK)?;
    let closed = wfah
        .entries()
        .iter()
        .any(|e| e.seq > last_fork.seq && CLOSERS.contains(&e.action.as_str()));
    if closed {
        return None;
    }
    let first_branch = fork_branches(last_fork).first()?.as_str()?;
    Some(round_of(wfah, first_branch))
}

/// `$valid` — defterin ELENMİŞ görünümü.
///
/// **ŞU AN YALNIZ KURAL 5 uygulanır.** Kalan dört kural (`Ç3`/`Ç4` kol iptali,
/// `Ç4-EK` geri gönderme penceresi, `Ç13` sahiplik satırları, `K7` tekilleştirme),
/// hesaplanan alanlar ve ZEN yüzeyi `E05` ile gelir — o iş bu fonksiyonu genişletir,
/// yenisini yazmaz.
pub fn derive_valid(wfah: &Wfah) -> Vec<&WfahEntry> {
    wfah.entries()
        .iter()
        .filter(|row| !eliminated_by_round(wfah, row))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::Actor;
    use serde_json::json;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "system".into(),
        }
    }

    fn row(seq: u32, action: &str, branch: Option<&str>, input: Option<Value>) -> WfahEntry {
        WfahEntry {
            seq,
            action: action.into(),
            actor: actor(),
            input,
            applied_at: chrono::Utc::now(),
            from_node: None,
            to_node: None,
            branch_entry: branch.map(str::to_string),
            branch_round: branch.map(|_| 1),
        }
    }

    fn fork(seq: u32) -> WfahEntry {
        row(
            seq,
            FORK,
            None,
            Some(json!({"branches": ["hukuk", "finans"]})),
        )
    }

    /// İki tur: birinci turun onayları elenir, ikinci turunkiler kalır.
    #[test]
    fn second_round_eliminates_first_round_rows() {
        let wfah = Wfah(vec![
            row(1, "basvuru", None, None),
            fork(2),
            row(3, "hukuk_onay", Some("hukuk"), None),
            row(4, "finans_onay", Some("finans"), None),
            row(5, "_join", None, None),
            row(6, "geri_gonder", None, None),
            fork(7),
            row(8, "hukuk_onay", Some("hukuk"), None),
        ]);
        let valid: Vec<u32> = derive_valid(&wfah).iter().map(|e| e.seq).collect();
        assert_eq!(valid, vec![1, 2, 5, 6, 7, 8]);
    }

    /// Tek turda hiçbir kol satırı elenmez.
    #[test]
    fn single_round_keeps_every_row() {
        let wfah = Wfah(vec![
            fork(1),
            row(2, "hukuk_onay", Some("hukuk"), None),
            row(3, "finans_onay", Some("finans"), None),
        ]);
        assert_eq!(derive_valid(&wfah).len(), 3);
    }

    /// Başka bir fork'un turu bu kolun satırlarını elemez (çapa kol kimliğidir).
    #[test]
    fn another_forks_round_does_not_eliminate() {
        let wfah = Wfah(vec![
            row(1, FORK, None, Some(json!({"branches": ["hukuk"]}))),
            row(2, "hukuk_onay", Some("hukuk"), None),
            row(3, "_join", None, None),
            row(4, FORK, None, Some(json!({"branches": ["teknik"]}))),
            row(5, "teknik_onay", Some("teknik"), None),
        ]);
        assert_eq!(derive_valid(&wfah).len(), 5);
    }

    #[test]
    fn round_counts_per_fork_and_starts_at_one() {
        let wfah = Wfah(vec![fork(1), row(2, "_join", None, None), fork(3)]);
        assert_eq!(round_of(&wfah, "hukuk"), 2);
        assert_eq!(round_of(&wfah, "teknik"), 0);
    }

    #[test]
    fn live_round_is_none_outside_parallel_mode() {
        assert_eq!(live_round(&Wfah(vec![row(1, "basvuru", None, None)])), None);
        let closed = Wfah(vec![fork(1), row(2, "_join", None, None)]);
        assert_eq!(live_round(&closed), None);
        let collapsed = Wfah(vec![fork(1), row(2, "_collapse", None, None)]);
        assert_eq!(live_round(&collapsed), None);
    }

    #[test]
    fn live_round_tracks_the_open_fork() {
        let first = Wfah(vec![fork(1), row(2, "hukuk_onay", Some("hukuk"), None)]);
        assert_eq!(live_round(&first), Some(1));
        let second = Wfah(vec![fork(1), row(2, "_join", None, None), fork(3)]);
        assert_eq!(live_round(&second), Some(2));
    }
}
