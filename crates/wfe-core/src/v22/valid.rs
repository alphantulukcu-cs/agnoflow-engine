//! `$valid` — ELENMİŞ geçmiş (WFD v2.3, K7–K9, §3.1).
//!
//! Ham `$wfah` bir DEFTERDİR: iptal edilmiş kolun onayı da, geri gönderme öncesi
//! turun onayı da orada durur. `count($wfah, …) >= 2` bu yüzden yanlış cevap
//! üretiyor. `$valid` aynı satırları elemeden geçirir; **saklanmaz**, her okumada
//! defterden türetilir (§3.1 "Saflık" — tek girdi defterdir).
//!
//! ## Eleme kuralları — BEŞ sebep (`A04`/S5 ile birebir)
//!
//! | `InvalidReason` | Kaynak karar |
//! |---|---|
//! | `branch_cancelled` | `Ç3`+`Ç4` — iptal edilen kolun satırları (`seq` YÖNLÜ) |
//! | `branch_superseded` | `Ç4-EK`/S4 — varmış kolun onayı geçersizleşir |
//! | `sent_back_window` | `Ç4-EK` — geri gönderme penceresi (İKİ dallı) |
//! | `ownership` | `Ç13` — sahiplik satırları eşik sayımına girmez |
//! | `old_round` | `E14`/S1 — kolunun yaşayan turundan önce yazılmış satır |
//!
//! **Altıncı sebep YOKTUR.** `K7` (tekilleştirme) bir eleme kuralı DEĞİLDİR:
//! `first_by_*_at_node` satır başına boolean'dır, satır DÜŞÜRMEZ (`A04`/S5).
//!
//! Kuralların TEK girdisi DEFTERDİR (`wfe_branch` tablosu DEĞİL — AND-join'de o
//! satırlar silinir, quorum'da `cancelled` kalır; naif bir uygulama AND yolunda
//! hiçbir şey bulamaz). Bu, `sim.rs`in DB'siz koşabilmesinin de tek sebebi.
//!
//! Kural setinin kendisi **BELGE SÜRÜMÜNE bağlıdır**; dikiş `ValidRules::for_version`.
//! Bugün tek sürüm var, ama dikiş baştan açıktır (§3.1 "Saflık").
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

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use crate::types::wfah::{Wfah, WfahEntry};
use crate::types::wfd_v22::{GlobalAction, Wfd, Wft};
use crate::v22::wfah_kind::parse_marker;

/// Fork marker'ı — kolları YARATAN satır. Adı sözleşmedir (Değişmez #2).
const FORK: &str = "_fork";
/// Paralel modu KAPATAN marker'lar. `_join` AND/quorum join'de (adapter + sim),
/// `_collapse` collapse / terminal / failed / terminated / quorum yollarında yazılır
/// (`pipeline::stage_parallel_markers`). İkisi birlikte paralel modu bitiren TÜM
/// yolları kapsar.
const CLOSERS: [&str; 2] = ["_join", "_collapse"];
/// Kol iptali / onay geçersizleşmesi marker'ları (`Ç3`) ve collapse manşeti.
/// Adlar sözleşmedir (Değişmez #2); eleme kuralı 1'in ÇAPASI bunlardır.
const BRANCH_CANCELLED: &str = "_branch_cancelled";
const BRANCH_SUPERSEDED: &str = "_branch_superseded";
const COLLAPSE: &str = "_collapse";
/// `Ç4-EK`/S4'ün iptal nedeni — geri gönderme kaynaklı collapse.
const SENT_BACK: &str = "sent_back";

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

// ------------------------------------------------------------------ sebep kümesi

/// Bir satırın `$valid`den NEDEN elendiği — **kapalı liste, BEŞ değer** (`A04`/S5).
///
/// Değer kümesinin TEK kaynağı burasıdır; portal kendi listesini icat etmez. `A04`
/// bunu `WfahView.invalid_reason` ile API'ye açar (`None` = satır GEÇERLİ); o iş
/// AYRI bir issue'dur ve bu enum'u ikinci kez tanımlamaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidReason {
    /// `Ç3`+`Ç4`: satır, iptal edilmiş bir kolda ve iptal marker'ından ÖNCE yazıldı.
    BranchCancelled,
    /// `Ç4-EK`/S4: kol join'e VARMIŞTI ama onayı geçersizleşti.
    BranchSuperseded,
    /// `Ç4-EK`: satır bir geri gönderme PENCERESİNİN içinde kaldı.
    SentBackWindow,
    /// `Ç13`: sahiplik olayı — aksiyon değildir, eşik sayımına girmez.
    Ownership,
    /// `E14`/S1: satır kolunun YAŞAYAN turundan önce yazıldı.
    OldRound,
}

// ---------------------------------------------------------------- kural seti

/// Eleme kuralı setinin **belge sürümüne bağlı** hâli (§3.1 "Saflık" dikişi).
///
/// Bugün tek sürüm var; `for_version` yine de baştan açıldı çünkü kural değişimi
/// GERİYE UYUM değil, İLERİYE dönük sürümlemedir: yayınlanmış bir belgenin sayım
/// semantiği motor güncellendiği için sessizce değişemez.
///
/// Setin taşıdığı tek WFD bilgisi geri gönderme aksiyonlarının kümesidir: `Ç4-EK`
/// penceresini AÇAN satırı ve `#.is_send_back` alanını bu küme belirler. Ölçüt
/// **AD DEĞİL YAPIDIR** — aksiyonun `wft`i `{targets}` formunda mı (§3.1). Editör
/// dördüncü, beşinci geri göndermeyi üretse de ifade dokunulmadan doğru kalır.
#[derive(Debug, Clone, Default)]
pub struct ValidRules {
    send_back_actions: BTreeSet<String>,
}

/// WF Admin'in geri gönderme yolu mu — `global_action_marker` bu adları yazar
/// (`admin:send_back` / `admin:send_to_start`).
///
/// WFD taramasından BAĞIMSIZ, kural setine göre DEĞİŞMEZ: bu adlar motorun kendi
/// sözleşmesidir (Değişmez #2), bir belgeden türemez. Admin yolunun WFD'de aksiyon
/// kaydı YOKTUR — §3.1'in yapısal ölçütü (*"aksiyonun `wft`i `{targets}` mi"*) onu
/// göremez — ama akış GERÇEKTEN geri gider ve `Ç4-EK`/S5 admin geri göndermeyi
/// paralel modda açıkça serbest bıraktı. Kural 2 bu satırları görmezse admin
/// geri göndermesi pencere AÇMAZ ve eşik sayımı eski turun onaylarını sessizce
/// saymaya devam eder — v2.3'ün B ekseninin düzeltmek için var olduğu hatanın aynısı.
fn is_admin_send_back(action: &str) -> bool {
    [GlobalAction::SendBack, GlobalAction::SendToStart]
        .iter()
        .any(|g| action.strip_prefix("admin:") == Some(g.as_str()))
}

impl ValidRules {
    /// §3.1 dikişi. Sürüm dallanması dokümandan okunur; bugün tek dal var ve
    /// **bilinmeyen sürüm de o dala düşer** — belge yükleme kapısı (`Wfd::from_*`)
    /// tanınmayan `wfd_version`'ı zaten reddediyor, burada ikinci bir kapı kurmak
    /// aynı soruyu iki yerde sorardı.
    pub fn for_version(wfd: &Wfd) -> Self {
        match wfd.wfd_version.as_str() {
            _ => Self {
                send_back_actions: send_back_action_keys(wfd),
            },
        }
    }

    /// `#.is_send_back` — satırın aksiyonu bir geri gönderme mi.
    pub fn is_send_back(&self, action: &str) -> bool {
        self.send_back_actions.contains(action) || is_admin_send_back(action)
    }
}

/// `wft`i `{targets}` formunda olan aksiyon anahtarları (admin yolu ayrı — bkz.
/// `is_admin_send_back`).
///
/// v2.3 yönlendirme kuralını aksiyonun kendi kaydına indirecek (`Ç5`, ayrı iş);
/// o iniş olduğunda bu tarama `wfd.actions` üzerinden yürür ve kural DEĞİŞMEZ —
/// sorulan soru "bu aksiyonun `wft`i `{targets}` mi" olarak kalır.
fn send_back_action_keys(wfd: &Wfd) -> BTreeSet<String> {
    wfd.transitions
        .iter()
        .filter(|t| matches!(t.wft, Wft::SendBack { .. }))
        .map(|t| t.action.clone())
        .collect()
}

// ------------------------------------------------------------------ eleme

/// **Eleme kuralı 1 (KOL İPTALİ / GEÇERSİZLEŞME).** `seq` YÖNLÜDÜR: bir
/// `_branch_cancelled` / `_branch_superseded` satırı yalnız KENDİ `seq`'inden
/// ÖNCEKİ aynı `branch_entry`'li satırları eler (`Ç4`/S1).
///
/// Yönsüz bir kural, aynı fork'a ikinci kez girildiğinde (`Ç4-EK`/S4) ikinci turun
/// satırlarını birinci turun iptal marker'ıyla elerdi.
///
/// Marker'ın KENDİSİ elenmez (`seq` eşit, küçük değil) — denetim satırı kalır.
/// Quorum join'de eşiğin ÜYESİ olan varmış kardeşler `superseded` işaretlenMEDİĞİ
/// için (`pipeline::stage_parallel_markers`) onayları da elenmez.
fn eliminated_by_branch_marker(wfah: &Wfah, row: &WfahEntry) -> Option<InvalidReason> {
    let entry = row.branch_entry.as_deref()?;
    wfah.entries()
        .iter()
        .filter(|m| m.branch_entry.as_deref() == Some(entry) && m.seq > row.seq)
        .find_map(|m| match m.action.as_str() {
            BRANCH_CANCELLED => Some(InvalidReason::BranchCancelled),
            BRANCH_SUPERSEDED => Some(InvalidReason::BranchSuperseded),
            _ => None,
        })
}

/// **Eleme kuralı 2 (GERİ GÖNDERME PENCERESİ).** `Ç4-EK` ile İKİ DALLIDIR.
///
/// Hedef T'ye geri gönderen satır S ise pencere `(sol_kenar, S.seq)` AÇIK
/// aralığıdır; sol kenar = S'den önceki `to_node == T` olan SON satırın `seq`'i,
/// yoksa 0 (§3.1: *"akışın T'ye en son girişinden bu geri gönderme satırına
/// kadar"*). Penceresi ÖNCESİ korunur.
///
/// İki dal:
/// * hedef **kol içinde** → pencere o kolun satırlarıyla sınırlıdır (aynı
///   `branch_entry`); `seq` WFE genelinde monotonik olduğu için kardeş kolun
///   satırları aralığa girer ama o kolun işi geri gönderilmemiştir,
/// * hedef **fork öncesinde** → geri gönderme bir COLLAPSE'tır ve pencere TÜM
///   kolları kapsar. Ayrımın ölçütü DEFTERDEDİR: aynı commit'in marker bloğunda
///   `reason: "sent_back"` taşıyan bir `_collapse` satırı var mı. Bu satırı
///   `Ç4-EK` yazacak; o iş inmeden dal sessizce hiç tetiklenmez (kural VERİ olarak
///   alınır, `E05`).
fn eliminated_by_send_back(
    wfah: &Wfah,
    rules: &ValidRules,
    row: &WfahEntry,
) -> Option<InvalidReason> {
    for s in wfah.entries() {
        if s.seq <= row.seq || !rules.is_send_back(&s.action) {
            continue;
        }
        let Some(target) = s.to_node.as_deref() else {
            continue;
        };
        let left = wfah
            .entries()
            .iter()
            .filter(|r| r.seq < s.seq && r.to_node.as_deref() == Some(target))
            .map(|r| r.seq)
            .max()
            .unwrap_or(0);
        if row.seq <= left {
            continue;
        }
        // Kol içi geri gönderme yalnız KENDİ kolunu eler; collapse tüm kolları.
        if s.branch_entry.is_some()
            && !is_collapsing_send_back(wfah, s)
            && row.branch_entry != s.branch_entry
        {
            continue;
        }
        return Some(InvalidReason::SentBackWindow);
    }
    None
}

/// Geri gönderme satırı paralel modu KAPATTI mı — aynı commit'in marker bloğunda
/// `reason: "sent_back"` taşıyan bir `_collapse` satırı var mı (`Ç4-EK`/S4).
///
/// "Aynı commit" = S'den sonraki, ilk AKSİYON satırına kadar olan marker dizisi;
/// `stage_parallel_markers` manşeti aksiyon satırının hemen ardına stage'liyor.
fn is_collapsing_send_back(wfah: &Wfah, s: &WfahEntry) -> bool {
    wfah.entries()
        .iter()
        .filter(|m| m.seq > s.seq)
        .take_while(|m| !parse_marker(&m.action).kind.is_action())
        .any(|m| {
            m.action == COLLAPSE
                && m.input
                    .as_ref()
                    .and_then(|i| i.get("reason"))
                    .and_then(Value::as_str)
                    == Some(SENT_BACK)
        })
}

/// Satır `$valid`den elendi mi — elendiyse SEBEBİYLE.
///
/// Sebep, sabit bir sırada İLK eşleşen kuraldır: satıra ÖZGÜ olan (sahiplik) → tur →
/// kol marker'ı → pencere. `$valid` KÜMESİ bu sıradan bağımsızdır (hepsi eler);
/// sıralama yalnız `A04`ün göstereceği sebebi belirler ve en dar/en açıklayıcı
/// olandan başlar.
pub fn invalid_reason(wfah: &Wfah, rules: &ValidRules, row: &WfahEntry) -> Option<InvalidReason> {
    if parse_marker(&row.action).kind.is_ownership() {
        return Some(InvalidReason::Ownership);
    }
    if eliminated_by_round(wfah, row) {
        return Some(InvalidReason::OldRound);
    }
    eliminated_by_branch_marker(wfah, row).or_else(|| eliminated_by_send_back(wfah, rules, row))
}

/// `$valid` — defterin ELENMİŞ görünümü. **Saklanmaz**, her okumada türetilir.
///
/// `seq` BOŞLUKLUDUR: `len(derive_valid(w)) != son seq` (A3, belgelenmiş).
pub fn derive_valid<'w>(wfah: &'w Wfah, rules: &ValidRules) -> Vec<&'w WfahEntry> {
    wfah.entries()
        .iter()
        .filter(|row| invalid_reason(wfah, rules, row).is_none())
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

    /// WFD'siz kural seti — geri gönderme aksiyonu tanımadan (yalnız admin
    /// marker'ları) koşar. Pencere testleri kendi setini kurar.
    fn rules() -> ValidRules {
        ValidRules::default()
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
        let valid: Vec<u32> = derive_valid(&wfah, &rules()).iter().map(|e| e.seq).collect();
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
        assert_eq!(derive_valid(&wfah, &rules()).len(), 3);
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
        assert_eq!(derive_valid(&wfah, &rules()).len(), 5);
    }

    /// Hareket taşıyan satır kurucusu — pencere kuralı `to_node`a bakar.
    fn moved(seq: u32, action: &str, from: &str, to: &str, branch: Option<&str>) -> WfahEntry {
        WfahEntry {
            from_node: Some(from.into()),
            to_node: Some(to.into()),
            ..row(seq, action, branch, None)
        }
    }

    /// Kural 1 — iptal marker'ı KENDİ `seq`'inden ÖNCEKİ kol satırlarını eler,
    /// sonrakileri ELEMEZ (`Ç4`: `seq` yönlü), kardeş kola da DOKUNMAZ.
    #[test]
    fn cancelled_branch_eliminates_only_its_earlier_rows() {
        let wfah = Wfah(vec![
            fork(1),
            row(2, "hukuk_onay", Some("hukuk"), None),
            row(3, "finans_onay", Some("finans"), None),
            row(4, "_branch_cancelled", Some("hukuk"), None),
        ]);
        assert_eq!(
            invalid_reason(&wfah, &rules(), &wfah.0[1]),
            Some(InvalidReason::BranchCancelled)
        );
        // Kardeş kol etkilenmez.
        assert_eq!(invalid_reason(&wfah, &rules(), &wfah.0[2]), None);
        // Marker'ın KENDİSİ denetim satırı olarak kalır.
        assert_eq!(invalid_reason(&wfah, &rules(), &wfah.0[3]), None);
    }

    /// Varmış kolun onayı geçersizleşirse sebep `branch_superseded`tir — iki sebep
    /// AYRI değerlerdir (`A04`/S5), tek bir "kol düştü" değerine katlanmaz.
    #[test]
    fn superseded_branch_has_its_own_reason() {
        let wfah = Wfah(vec![
            fork(1),
            row(2, "hukuk_onay", Some("hukuk"), None),
            row(3, "_branch_superseded", Some("hukuk"), None),
        ]);
        assert_eq!(
            invalid_reason(&wfah, &rules(), &wfah.0[1]),
            Some(InvalidReason::BranchSuperseded)
        );
    }

    /// Kural 3 — sahiplik satırları (`Ç13`) eşik sayımına GİRMEZ. Ölçüt SINIFTIR,
    /// ad listesi değildir.
    #[test]
    fn ownership_rows_are_eliminated() {
        let wfah = Wfah(vec![
            row(1, "claim_taken:self__memur", None, None),
            row(2, "onayla", None, None),
            row(3, "claim_released:self__memur", None, None),
        ]);
        assert_eq!(
            invalid_reason(&wfah, &rules(), &wfah.0[0]),
            Some(InvalidReason::Ownership)
        );
        assert_eq!(
            invalid_reason(&wfah, &rules(), &wfah.0[2]),
            Some(InvalidReason::Ownership)
        );
        assert_eq!(invalid_reason(&wfah, &rules(), &wfah.0[1]), None);
        assert_eq!(derive_valid(&wfah, &rules()).len(), 1);
    }

    /// Diğer marker satırları (escalation, trigger, kol olayları) `$valid`de KALIR —
    /// altıncı bir eleme kuralı yoktur. Tasarımcı onları `#.kind != "action"` ile ayırır.
    #[test]
    fn non_ownership_markers_stay_in_valid() {
        let wfah = Wfah(vec![
            row(1, "onayla", None, None),
            row(2, "escalate:self__memur:0", None, None),
            row(3, "trigger:use_skor", None, None),
            row(4, "timeout:deadline", None, None),
        ]);
        assert_eq!(derive_valid(&wfah, &rules()).len(), 4);
    }

    /// Kural 2 — pencere `(sol_kenar, S.seq)` AÇIK aralığıdır: T'ye en son girişten
    /// ÖNCEKİ satırlar KORUNUR, sonrakiler düşer, geri gönderme satırının kendisi kalır.
    #[test]
    fn send_back_window_drops_only_the_rows_after_the_last_entry() {
        let wfah = Wfah(vec![
            // 1: akış memur'a girer (T = self__memur)
            moved(1, "basvuru", "start", "self__memur", None),
            // 2: memur'da iş yapılır, komiteye çıkar
            moved(2, "memur_onay", "self__memur", "self__komite", None),
            // 3: komite geri gönderir → pencere (1, 3) = yalnız 2. satır
            moved(3, "geri_gonder", "self__komite", "self__memur", None),
            moved(4, "memur_onay", "self__memur", "self__komite", None),
        ]);
        let r = ValidRules {
            send_back_actions: ["geri_gonder".to_string()].into_iter().collect(),
        };
        assert_eq!(invalid_reason(&wfah, &r, &wfah.0[0]), None, "pencere öncesi korunur");
        assert_eq!(
            invalid_reason(&wfah, &r, &wfah.0[1]),
            Some(InvalidReason::SentBackWindow)
        );
        assert_eq!(invalid_reason(&wfah, &r, &wfah.0[2]), None, "geri gönderme satırı kalır");
        assert_eq!(invalid_reason(&wfah, &r, &wfah.0[3]), None);
        // Eşik artık DOĞRU sayar: iki `memur_onay` satırından yalnız biri geçerli.
        let valid = derive_valid(&wfah, &r);
        assert_eq!(
            valid.iter().filter(|e| e.action == "memur_onay").count(),
            1,
            "ham defterde 2, $valid'de 1 olmalı"
        );
    }

    /// Kural 2 / dal A — hedef KOL İÇİNDEyse pencere o kolla sınırlıdır; kardeş
    /// kolun satırı aralığa `seq` olarak girse de ELENMEZ.
    #[test]
    fn in_branch_send_back_window_does_not_touch_the_sibling() {
        let wfah = Wfah(vec![
            fork(1),
            moved(2, "hukuk_giris", "hukuk", "hukuk_kontrol", Some("hukuk")),
            moved(3, "finans_onay", "finans", "finans_son", Some("finans")),
            moved(4, "geri_gonder", "hukuk_kontrol", "hukuk", Some("hukuk")),
        ]);
        let r = ValidRules {
            send_back_actions: ["geri_gonder".to_string()].into_iter().collect(),
        };
        assert_eq!(
            invalid_reason(&wfah, &r, &wfah.0[1]),
            Some(InvalidReason::SentBackWindow)
        );
        assert_eq!(
            invalid_reason(&wfah, &r, &wfah.0[2]),
            None,
            "kardeş kolun onayı kol içi pencereye girmez"
        );
    }

    /// Kural 2 / dal B — geri gönderme bir COLLAPSE ise (`reason: "sent_back"`)
    /// pencere TÜM kolları kapsar.
    #[test]
    fn collapsing_send_back_window_covers_every_branch() {
        let wfah = Wfah(vec![
            fork(1),
            moved(2, "hukuk_giris", "hukuk", "hukuk_kontrol", Some("hukuk")),
            moved(3, "finans_onay", "finans", "finans_son", Some("finans")),
            moved(4, "geri_gonder", "hukuk_kontrol", "self__memur", Some("hukuk")),
            row(5, "_collapse", None, Some(json!({"reason": "sent_back"}))),
        ]);
        let r = ValidRules {
            send_back_actions: ["geri_gonder".to_string()].into_iter().collect(),
        };
        assert_eq!(
            invalid_reason(&wfah, &r, &wfah.0[2]),
            Some(InvalidReason::SentBackWindow),
            "collapse penceresi kardeş kolu da eler"
        );
    }

    /// Admin geri göndermesi de pencere AÇAR — WFD'de aksiyon kaydı olmadığı için
    /// yapısal ölçüt onu göremez, ama akış gerçekten geri gider (`Ç4-EK`/S5).
    #[test]
    fn admin_send_back_opens_a_window() {
        let wfah = Wfah(vec![
            moved(1, "basvuru", "start", "self__memur", None),
            moved(2, "memur_onay", "self__memur", "self__komite", None),
            moved(3, "admin:send_back", "self__komite", "self__memur", None),
        ]);
        assert_eq!(
            invalid_reason(&wfah, &rules(), &wfah.0[1]),
            Some(InvalidReason::SentBackWindow)
        );
    }

    /// `seq` BOŞLUKLUDUR (A3): `len($valid)` son `seq`e eşit DEĞİLDİR.
    #[test]
    fn valid_seq_is_gappy() {
        let wfah = Wfah(vec![
            fork(1),
            row(2, "hukuk_onay", Some("hukuk"), None),
            row(3, "_branch_cancelled", Some("hukuk"), None),
        ]);
        let valid = derive_valid(&wfah, &rules());
        let seqs: Vec<u32> = valid.iter().map(|e| e.seq).collect();
        assert_eq!(seqs, vec![1, 3]);
        assert_ne!(valid.len() as u32, wfah.entries().last().unwrap().seq);
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
