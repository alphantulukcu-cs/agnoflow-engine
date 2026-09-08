//! WFAH satırının SINIFI — ad→sınıf kuralının TEK yeri (`R04`, `E07`).
//!
//! # Neden çekirdekte
//!
//! Sınıf **ADDAN türetilir**: marker adları sözleşmedir (Değişmez #2), dolayısıyla
//! saklanacak bir veri yoktur ve `WfahEntry`'ye sınıf ALANI EKLENMEZ (`R04`) —
//! alan eklemek `Ç2`nin `to_node` bedelini (her üreticinin doğru doldurması + eski
//! satırlarda NULL) türetilebilir bir bilgi için tekrarlamak olurdu.
//!
//! Kural `crates/wfe/src/executor.rs`ten buraya indi çünkü İKİ çekirdek tüketicisi
//! var ve ikisi de adapter'ı göremez:
//!
//! * `v22::eval::project_entry` — `#.kind` alanı (`E05`),
//! * `v22::valid` — `$prev`/`$first` elemesi (`R04`) ve sahiplik elemesi (`Ç13`).
//!
//! `wfe` adapteri tipi **re-export** eder; `WfahView` ve API görünümü değişmez.
//! İkinci bir ad→sınıf eşlemesi YAZILMAZ (`E05`/S4).

use serde::Serialize;

/// Bir WFAH satırının NE OLDUĞU — **kapalı liste, 15 varyant** (`E07`/S1).
///
/// Motorun kendi marker adları (`_branch_cancelled`, `escalate:<node>:<idx>`,
/// `call:<key>/<action>` …) DEĞİŞMEZ: yayınlanmış akışlar `count($wfah, #.action ==
/// ...)` ile karar veriyor. Değişen yalnız GÖRÜNÜMDÜR — sınıflandırma burada yapılır,
/// istemciye ham metin ASLA verilmez.
///
/// `serde(rename_all = "snake_case")` serileşmesi **sözleşmedir**: `E05` bu 15 adı
/// `#.kind` ile tasarımcıya açtı, `P04` de `Record<WfahKind, …>` anahtar kümesi olarak
/// kullanır. Bir varyantı yeniden adlandırmak yayınlanmış `#.kind == "…"` ifadelerini
/// SESSİZCE hep-false yapar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WfahKind {
    /// İnsan (ya da WF Admin) eliyle alınan normal aksiyon — varsayılan sınıf.
    Action,
    /// SLA-3: akış deadline'ı doldu.
    Deadline,
    Escalation,
    EscalationSkipped,
    /// Ç13: sahiplik DOĞDU — `claim_taken:<node>`. Satırı **`E12`** yazar; o iş
    /// inmeden `#.kind == "claim_taken"` hep-false okur (`E07`, FEDA EDİLENLER).
    ClaimTaken,
    /// Ç1-EK: sahiplik DÜŞTÜ — `claim_released:<node>`. Sebep payload'daki `reason`
    /// alanındadır (`timeout` | `grant_guard_false` | …), ADda değil.
    ClaimReleased,
    Trigger,
    CallReturn,
    /// Alt akış geçmişi çağıranın defterine sığmadı, kırpıldı.
    CallTruncated,
    Fork,
    BranchArrived,
    /// Kollar birleşti. `_join` marker'ını **çekirdek YAZMAZ** — AND/quorum join'de
    /// son varış doğrulamasıyla aynı transaction'da `wfe::wfe_adapter` yazar, sim de
    /// kendi store'unda yazar (Ç2'nin dokümante ettiği istisna). Varyant bu yüzden
    /// yaşıyor; "bütünlük için duruyor" diyen eski yorum YANLIŞTI (`E07`/S2).
    Join,
    Collapse,
    BranchCancelled,
    BranchSuperseded,
}

/// Bir WFAH satırının ROZET GRUBU — `WfahKind`in SAF fonksiyonu (`P04`).
///
/// # Neden motorda
///
/// Bu, aynı 15 `kind`in **ikinci sınıflandırmasıdır**. Portalda `wfahBadgeGroup` diye
/// elle yazılıyordu; `E07`nin kapalı listesini istemcide aynalamak demekti ve yeni bir
/// varyant eklendiğinde hiçbir derleyici orayı göstermiyordu. `api-contract-v2` §2d'nin
/// motora taşıdığı işin unutulmuş parçasıydı.
///
/// İstemcide kalan tek şey `kind` → **ikon/renk** seçimidir; sınıflandırmanın tamamı
/// burada. Portal metin de ÜRETMEZ — etiketler motordan gelir.
///
/// `serde` serileşmesi `WfahView.group` olarak wire'a çıkar ve SÖZLEŞMEDİR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WfahGroup {
    /// Süre olayları: akış deadline'ı ve escalation kademeleri.
    Sla,
    /// `Ç13`: sahiplik DOĞDU ya da DÜŞTÜ. Ayrı bir grup olmasının gerekçesi Ç13'ün
    /// gerekçesiyle aynı — *"yeni bir olay SINIFI gerçekten var"*.
    ///
    /// ⚠️ `claim_released` `Sla`da KALAMAZDI: `E12` ile artık `self` / `admin` /
    /// `taken_by_other` sebeplerini de taşıyor, yani SLA olayı OLMAYAN satırlar
    /// SLA rozetiyle görünürdü.
    Ownership,
    /// Kolların açılması, varması, birleşmesi.
    Parallel,
    /// Paralel modu KAPATAN olaylar — `collapse` ve iki kol düşme biçimi.
    ///
    /// Varyant adı `CollapseGroup`, çünkü `Collapse` adı `WfahKind`de zaten var ve
    /// iki enum aynı modülde `use ...::*` ile birlikte kullanılıyor. Wire değeri
    /// `"collapse"` — okuyucu için fark YOK.
    #[serde(rename = "collapse")]
    CollapseGroup,
    /// Alt akış çağrısının kapanışı / kırpılması.
    Call,
    /// Rozet TAŞIMAYAN satırlar: insan aksiyonu ve otomatik işlem.
    ///
    /// Adın sonundaki alt çizgi `Option::None` ile karışmasın diye; wire değeri
    /// `"none"`.
    #[serde(rename = "none")]
    None_,
}

impl WfahKind {
    /// Bu satırın rozet grubu (`P04`). Jokersiz `match`: yeni bir `WfahKind` varyantı
    /// eklendiğinde derleyici BURAYI gösterir ve grup sorusu bir kez cevaplanır.
    pub fn group(self) -> WfahGroup {
        match self {
            WfahKind::Deadline | WfahKind::Escalation | WfahKind::EscalationSkipped => {
                WfahGroup::Sla
            }
            WfahKind::ClaimTaken | WfahKind::ClaimReleased => WfahGroup::Ownership,
            WfahKind::Fork | WfahKind::BranchArrived | WfahKind::Join => WfahGroup::Parallel,
            WfahKind::Collapse | WfahKind::BranchCancelled | WfahKind::BranchSuperseded => {
                WfahGroup::CollapseGroup
            }
            WfahKind::CallReturn | WfahKind::CallTruncated => WfahGroup::Call,
            WfahKind::Action | WfahKind::Trigger => WfahGroup::None_,
        }
    }

    /// Bu satır bir AKSİYON satırı mı — `$prev`/`$first` elemesinin ölçütü (`R04`/S1).
    ///
    /// Aksiyon = WFD'nin `actions` kaydındaki bir adımın gerçekleştirilmesi (Ç13).
    /// Ölçüt SINIFTIR, ad listesi DEĞİLDİR: yeni bir marker türü eklendiğinde
    /// `WfahKind` varyantı derleyici tarafından zorlanır, güncellenecek liste yoktur.
    pub fn is_action(self) -> bool {
        matches!(self, WfahKind::Action)
    }

    /// Bu satır bir SAHİPLİK satırı mı (Ç13) — `$valid` eleme kuralı: sahiplik
    /// olayları eşik sayımına girmez.
    pub fn is_ownership(self) -> bool {
        matches!(self, WfahKind::ClaimTaken | WfahKind::ClaimReleased)
    }
}

/// Marker adının çözümlenmiş hâli — `WfahView`'ın metin ayrıştırma çekirdeği.
/// Saf (WFD'siz, `Ref`siz) tutulur ki birim testlenebilsin.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedMarker {
    pub kind: WfahKind,
    /// `kind: action` ise aksiyonun KENDİ adı (call öneki sökülmüş).
    pub action: Option<String>,
    pub node: Option<String>,
    pub step: Option<usize>,
    pub from_call: Option<String>,
}

/// Ham WFAH `action` adını sınıflandırır.
///
/// Ayrıştırma önek/desen tabanlıdır çünkü marker adlarının KENDİSİ sözleşmedir:
/// `escalate:` öneki bu sınıflandırma ve yayınlanmış `count($wfah, …)` sayımları için
/// ZORUNLUDUR (bkz. CLAUDE.md WF Admin bölümü). Önek `next_escalation`ın TABANINI
/// artık BELİRLEMEZ — R02'den beri taban `to_node != null` satırlardan gelir.
/// Tanınmayan her ad `Action`a düşer: bilinmeyen bir marker'ı "sistem" diye
/// etiketlemek, ham adı ekrana basmaktan daha yanıltıcı olurdu.
pub fn parse_marker(raw: &str) -> ParsedMarker {
    let plain = |kind: WfahKind| ParsedMarker {
        kind,
        action: None,
        node: None,
        step: None,
        from_call: None,
    };

    // Alt akış izdüşümü: `call:<key>/<action>` — önek SÖKÜLÜR, kalan ad kendi
    // kurallarıyla yeniden sınıflandırılır (alt akışın markerları da markerdır).
    if let Some(rest) = raw.strip_prefix("call:") {
        return match rest.split_once('/') {
            // `call:<key>/…` — kırpma işareti (bkz. pipeline `format!("{marker}/…")`).
            Some((key, "…")) => ParsedMarker {
                from_call: Some(key.to_string()),
                ..plain(WfahKind::CallTruncated)
            },
            Some((key, inner)) => ParsedMarker {
                from_call: Some(key.to_string()),
                ..parse_marker(inner)
            },
            // Önek tek başına = çağrının KAPANIŞ marker'ı (dönüş işlendi).
            None => ParsedMarker {
                from_call: Some(rest.to_string()),
                ..plain(WfahKind::CallReturn)
            },
        };
    }
    if let Some(rest) = raw.strip_prefix("escalate:") {
        // `<node>:<idx>` ya da `<node>:<idx>:skipped`
        let (body, kind) = match rest.strip_suffix(":skipped") {
            Some(b) => (b, WfahKind::EscalationSkipped),
            None => (rest, WfahKind::Escalation),
        };
        // Node anahtarı `:` içermez; sondaki alan adım numarasıdır.
        let (node, step) = match body.rsplit_once(':') {
            Some((n, idx)) => (Some(n.to_string()), idx.parse::<usize>().ok()),
            None => (Some(body.to_string()), None),
        };
        return ParsedMarker {
            node,
            step,
            ..plain(kind)
        };
    }
    // Ç1-EK: `claim_timeout:` → `claim_released:`. Sebep ADda değil payload'ın
    // `reason` alanındadır; SLA-1 dışı bırakma sebepleri (Ç9 grant guard) aynı adı
    // kullanır. Ç13: `claim_taken:` AYNI desendir — node anahtarı adın içinden sökülür.
    for (prefix, kind) in [
        ("claim_released:", WfahKind::ClaimReleased),
        ("claim_taken:", WfahKind::ClaimTaken),
    ] {
        if let Some(node) = raw.strip_prefix(prefix) {
            return ParsedMarker {
                node: Some(node.to_string()),
                ..plain(kind)
            };
        }
    }
    if raw.starts_with("trigger:") {
        return plain(WfahKind::Trigger);
    }
    match raw {
        "timeout:deadline" => plain(WfahKind::Deadline),
        "_fork" => plain(WfahKind::Fork),
        "_branch_arrived" => plain(WfahKind::BranchArrived),
        "_join" => plain(WfahKind::Join),
        "_collapse" => plain(WfahKind::Collapse),
        "_branch_cancelled" => plain(WfahKind::BranchCancelled),
        "_branch_superseded" => plain(WfahKind::BranchSuperseded),
        other => ParsedMarker {
            action: Some(other.to_string()),
            ..plain(WfahKind::Action)
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `serde` serileşmesi `#.kind` DEĞER KÜMESİDİR (`E05`) — 15 adın hepsi çivilenir.
    #[test]
    fn serialised_names_are_the_kind_value_set() {
        let all = [
            (WfahKind::Action, "action"),
            (WfahKind::Deadline, "deadline"),
            (WfahKind::Escalation, "escalation"),
            (WfahKind::EscalationSkipped, "escalation_skipped"),
            (WfahKind::ClaimTaken, "claim_taken"),
            (WfahKind::ClaimReleased, "claim_released"),
            (WfahKind::Trigger, "trigger"),
            (WfahKind::CallReturn, "call_return"),
            (WfahKind::CallTruncated, "call_truncated"),
            (WfahKind::Fork, "fork"),
            (WfahKind::BranchArrived, "branch_arrived"),
            (WfahKind::Join, "join"),
            (WfahKind::Collapse, "collapse"),
            (WfahKind::BranchCancelled, "branch_cancelled"),
            (WfahKind::BranchSuperseded, "branch_superseded"),
        ];
        assert_eq!(all.len(), 15, "kapalı liste 15 varyanttır (E07/S1)");
        for (kind, name) in all {
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(name));
        }
    }

    /// `P04` — **rozet grubu motora taşındı.** `WfahGroup`, `WfahKind`in SAF
    /// fonksiyonudur ve altı değer taşır. Portaldaki `wfahBadgeGroup` aynı 15 `kind`in
    /// İKİNCİ sınıflandırmasıydı; ikinci bir liste tutmak, `E07`nin kapalı listesini
    /// istemcide elle aynalamak demekti.
    #[test]
    fn every_kind_maps_to_exactly_one_group() {
        use WfahGroup::*;
        use WfahKind::*;
        let beklenen = [
            (Deadline, Sla),
            (Escalation, Sla),
            (EscalationSkipped, Sla),
            // ⚠️ `claim_released` `sla` grubunda KALAMAZ: `E12` ile artık `self` /
            // `admin` / `taken_by_other` sebeplerini de taşıyor, yani SLA olayı
            // OLMAYAN satırlar o grupta görünürdü.
            (ClaimTaken, Ownership),
            (ClaimReleased, Ownership),
            (Fork, Parallel),
            (BranchArrived, Parallel),
            (Join, Parallel),
            (Collapse, CollapseGroup),
            (BranchCancelled, CollapseGroup),
            (BranchSuperseded, CollapseGroup),
            (CallReturn, Call),
            (CallTruncated, Call),
            (Action, None_),
            (Trigger, None_),
        ];
        assert_eq!(beklenen.len(), 15, "kapalı liste 15 varyanttır (E07/S1)");
        for (kind, group) in beklenen {
            assert_eq!(kind.group(), group, "{kind:?} yanlış gruba düştü");
        }
    }

    /// Grup serileşmesi de SÖZLEŞMEDİR — `WfahView.group` olarak wire'a çıkıyor ve
    /// portal ona göre rozet seçiyor.
    #[test]
    fn group_names_are_the_wire_value_set() {
        let all = [
            (WfahGroup::Sla, "sla"),
            (WfahGroup::Ownership, "ownership"),
            (WfahGroup::Parallel, "parallel"),
            (WfahGroup::CollapseGroup, "collapse"),
            (WfahGroup::Call, "call"),
            (WfahGroup::None_, "none"),
        ];
        assert_eq!(all.len(), 6);
        for (g, name) in all {
            assert_eq!(serde_json::to_value(g).unwrap(), serde_json::json!(name));
        }
    }

    #[test]
    fn ownership_markers_are_classified() {
        let released = parse_marker("claim_released:self__memur");
        assert_eq!(released.kind, WfahKind::ClaimReleased);
        assert_eq!(released.node.as_deref(), Some("self__memur"));
        let taken = parse_marker("claim_taken:self__memur");
        assert_eq!(taken.kind, WfahKind::ClaimTaken);
        assert_eq!(taken.node.as_deref(), Some("self__memur"));
        assert!(taken.kind.is_ownership() && released.kind.is_ownership());
        assert!(!taken.kind.is_action());
    }

    /// Eski ad artık marker DEĞİLDİR — `Action`a düşer. Geriye uyum eşlemesi
    /// YAZILMAZ (Ç1-EK, Değişmez #9: ürün production'da değil).
    #[test]
    fn old_claim_timeout_name_is_no_longer_a_marker() {
        assert_eq!(parse_marker("claim_timeout:self__memur").kind, WfahKind::Action);
    }

    #[test]
    fn escalation_and_call_markers_keep_their_rules() {
        let esc = parse_marker("escalate:self__memur:1");
        assert_eq!(esc.kind, WfahKind::Escalation);
        assert_eq!(esc.step, Some(1));
        assert_eq!(
            parse_marker("escalate:self__memur:1:skipped").kind,
            WfahKind::EscalationSkipped
        );
        assert_eq!(parse_marker("call:krediler").kind, WfahKind::CallReturn);
        assert_eq!(parse_marker("call:krediler/…").kind, WfahKind::CallTruncated);
        // Alt akış AKSİYONU aksiyon KALIR (R04/S1) — `$prev` onu görür.
        let inner = parse_marker("call:krediler/onayla");
        assert_eq!(inner.kind, WfahKind::Action);
        assert_eq!(inner.action.as_deref(), Some("onayla"));
        assert_eq!(inner.from_call.as_deref(), Some("krediler"));
    }

    #[test]
    fn branch_markers_are_not_actions() {
        for raw in [
            "_fork",
            "_branch_arrived",
            "_join",
            "_collapse",
            "_branch_cancelled",
            "_branch_superseded",
            "timeout:deadline",
            "trigger:use_skor",
        ] {
            assert!(
                !parse_marker(raw).kind.is_action(),
                "'{raw}' aksiyon satırı sayılmamalı"
            );
        }
        assert!(parse_marker("basvuru").kind.is_action());
    }
}
