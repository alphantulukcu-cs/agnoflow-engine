//! Marker payload şekillerinin **TEK KAYNAĞI** (`P04`).
//!
//! # Neden bu dosya var
//!
//! Marker payload'larının şekli beş yerde ayrı ayrı yaşıyordu ve hiçbiri diğerini
//! zorlamıyordu: `pipeline.rs`in `json!{}` literalleri, `sim.rs`in ikinci üretimi,
//! portalın elle yazdığı `Wfah*Input` arayüzleri, `E05`in alan listesi ve
//! `decisions.md`in düzyazısı. Dört karar (`Ç3`, `Ç4-EK`, `Ç1-EK`, `Ç13`) artı `E12` ve
//! `E14` hep bu beş kopyayı **elle** senkronlama işiydi.
//!
//! Burası o kopyaları bire indirir. Marker bilgisinin tek yeri artık şu zincirdir:
//! **ad → sınıf (`WfahKind`) → grup (`WfahGroup`) → payload şekli (bu dosya) → etiket.**
//!
//! # Ayrımı neden `serde(tag = "kind")` TAŞIMIYOR
//!
//! `P04` etiketli birleşim öneriyordu ama disk iki engel gösterdi:
//!
//! 1. **`_collapse` payload'ının KENDİ `kind` alanı var** (`collapsed` / `sent_back` /
//!    `join_quorum` / `terminal` / `failed` / `terminated`) — serde etiketi onunla
//!    çakışırdı. O alanı yeniden adlandırmak portalı ve `$valid`in okuduğu yüzeyi
//!    kırardı.
//! 2. Etiket eklemek **saklanan** payload'ların şeklini değiştirirdi; ayrım zaten
//!    satırın `action` ADINDA duruyor (`parse_marker`) ve deftere ikinci kez yazmak
//!    aynı gerçeği iki yerde tutmak olurdu — bu kaydın kapatmaya çalıştığı hastalığın
//!    ta kendisi.
//!
//! Kararın hükmü *"tek ŞEKİL TANIMI"*dır, mekanizma değil: *"serde ile genelleştirme
//! direnç gösterirse … iki ayrı ŞEKİL TANIMI kabul edilmez."* Şekil burada TEK; ayrım
//! `WfahKind`ten sürülür (`from_row`). Sonuç olarak **saklanan payload'lar birebir
//! korunur** ve tip yine derleyici kalkanı verir.
//!
//! # Kapsam: yalnız MARKER satırları
//!
//! Aksiyon satırlarının `input`u bir marker payload'u DEĞİL, kullanıcının aksiyon
//! girdisidir (WFD'ye göre değişen serbest JSON). Onu bu birleşime sarmak
//! `#.input.<yol>` sayımlarını `#.input.input.<yol>`a çevirir ve yayınlanmış akışları
//! SESSİZCE bozardı — `$wfah` izdüşümü `input`u payload'ın KENDİSİ diye tanımlıyor.
//! Aksiyon satırları (ve `admin:*` global aksiyon satırları, ki onlar da `WfahKind`
//! tarafında `Action`a düşüyor) ham `input`larını korur.

use crate::v22::wfah_kind::WfahKind;
use serde::Serialize;
use serde_json::Value;

/// Bir MARKER satırının payload'ı — düğüm anahtarı cinsinden geneldir.
///
/// `N = String` çekirdekte (deftere yazılan ham anahtar), `N = Ref` API sınırında
/// (çözülmüş `{id, label}`). **Tek şekil tanımı, iki örnekleme.**
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum WfahPayload<N> {
    /// SLA-3: akış deadline'ı doldu.
    ///
    /// ⚠️ Zaman alanları `Value` taşır, `String`/`DateTime` DEĞİL — ve bu bilinçli:
    /// `chrono`nun serde biçimi (`…10:00:00Z`) ile `to_rfc3339()` (`…10:00:00+00:00`)
    /// AYRIŞIYOR. Tip bir dönüşüm dayatsaydı saklanan zaman damgaları sessizce
    /// değişirdi; üretici bugünkü ifadeyi AYNEN geçirir, şekil birebir korunur.
    Deadline { deadline: Option<Value> },

    /// SLA-2 kademesi ateşlendi. `grant` `Ç9`un yetki kuralı — `when` metni deftere
    /// AYNEN yazılır (`E13`: guard'ın ne olduğu audit izinde durur).
    Escalation { after: String, grant: GrantPayload },

    /// WF Admin kademeyi ELLE atladı.
    ///
    /// `skipped: true` alanı, sınıf (`WfahKind::EscalationSkipped`) zaten aynı şeyi
    /// söylediği için teknik olarak gereksiz — ama saklanan satırlarda DURUYOR ve
    /// bu kayıt şekli korumayı seçti (bkz. modül başlığı).
    EscalationSkipped { skipped: bool, after: String },

    /// `Ç13`/`E12`: sahiplik DOĞDU. Şekli `v22::ownership` üretir; burada `Value`
    /// olarak taşınır çünkü kapalı listeleri (via/authority) o modül sahiplenir —
    /// ikinci bir tanım yazmak bu kaydın yasakladığı şeydir.
    ClaimTaken(Value),

    /// `Ç1-EK`/`E12`: sahiplik DÜŞTÜ. Aynı gerekçe.
    ClaimReleased(Value),

    /// Otomatik işlem koştu. ÜÇ şekli var (başarı / yakalanmış hata / yakalanmamış),
    /// hepsi tek varyantta: alanların hangisinin dolu olduğu sonucu anlatır.
    Trigger {
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        handled: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        required: Option<bool>,
    },

    /// Alt akış kapandı.
    CallReturn {
        status: Option<String>,
        callee_wfe_id: Option<String>,
    },

    /// Alt akış geçmişi çağıranın defterine sığmadı.
    CallTruncated {
        callee_wfe_id: Option<String>,
        omitted: usize,
        reason: String,
    },

    /// Paralel kollar açıldı. Satırın KENDİSİ bir kolun içinde değildir (`Ç3`/`Ç4`).
    Fork {
        branches: Vec<N>,
        join: Value,
        join_mode: String,
        join_threshold: Option<u32>,
        join_when: Option<String>,
    },

    /// Kol join hedefine vardı.
    ///
    /// `claimed_by`, `Ç13`ün *"claim üç yoldan düşer"* okuma kuralının (c) ayağıdır:
    /// kol kapanışı ÖRTÜK bir bırakmadır ve kimin sahipliğinin düştüğünü YALNIZ bu
    /// alan söyler — `approved_by` eylemi YAPANı gösterir, sahibi değil (vekaleten
    /// alınmış kolda ikisi ayrışır).
    BranchArrived {
        branch_entry: Option<N>,
        at_node: N,
        approved_by: Value,
        /// Zaman alanları `Value` (bkz. `Deadline`).
        approved_at: Option<Value>,
        claimed_at: Option<Value>,
        claimed_by: Option<String>,
    },

    /// Kollar birleşti. Bugün payload TAŞIMAZ.
    Join,

    /// Paralel modu KAPATAN olayın MANŞETİ (`_collapse`) — bir kolun satırı değil,
    /// paralel modun tamamının özeti.
    ///
    /// ⚠️ İçindeki `kind` alanı bu birleşimin ayrımı DEĞİLDİR; olayın SEBEP TÜRÜdür
    /// (`collapse_to` / `sent_back` / `join_quorum` / `terminal` / …). Serde etiketi
    /// tam da bu yüzden kullanılamadı.
    Collapse(CollapsePayload<N>),

    /// DÜŞEN kolun satırı. Manşetle AYNI şekli TAŞIMAZ — kararın *"üçü aynı"*
    /// varsayımı diskte tutmadı: kol satırı `cancelled`/`superseded` listelerini
    /// taşımaz, onun yerine kendi kimliğini/konumunu ve iptal ANINDAKİ claim'ini
    /// taşır (`WOR-59`: adapter o alanları hemen ardından NULL'lar, tek kayıt yeri
    /// burasıdır).
    BranchCancelled(BranchDropPayload<N>),

    /// Varmış ama geçersizleşmiş kolun satırı. `cancelled`den farkı, onayın KİM
    /// tarafından ne zaman verildiğini de taşımasıdır — o onay artık sayılmıyor ve
    /// kimin onayının düştüğü yalnız buradan okunabilir.
    BranchSuperseded(BranchDropPayload<N>),

    /// Çözülemeyen ya da bu sürümün tanımadığı payload.
    ///
    /// **Geriye uyum kodu DEĞİLDİR** (Değişmez #9): okumanın TOPLAM FONKSİYON olması.
    /// Eski bir satır okuma yolunu patlatmaz, `Unknown` olarak geçer ve ekranda yalnız
    /// `label` görünür. Eski satırlar için okuyucu/dönüştürücü/migration YAZILMAZ.
    Unknown(Value),
}

/// `Ç9` grant kuralının payload karşılığı.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GrantPayload {
    pub c_a: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Paralel modu kapatan olayın MANŞETİ. Kol satırları AYRI şekildedir
/// (`BranchDropPayload`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CollapsePayload<N> {
    /// Olayın SEBEP TÜRÜ — `collapse_to` · `sent_back` · `join_quorum` · `terminal` ·
    /// `failed` · `terminated`. (Birleşimin ayrımı değildir; bkz. `WfahPayload::Collapse`.)
    pub kind: String,
    /// `$valid` eleme kuralı 2'nin collapse dalı tam olarak bu alanı arar.
    pub reason: String,
    /// Yalnız node hedefli collapse'ta anlamlı — terminal yollarında akış bir node'a
    /// GİTMEZ, sonucu `wfe.end_response` taşır.
    pub target: Option<N>,
    pub cancelled: Vec<N>,
    pub superseded: Vec<N>,
    /// `Ç4-EK`/S5: tetikleyicinin cinsi — `branch` · `admin` · `system`.
    pub trigger_kind: String,
    pub trigger_branch: Option<N>,
    pub trigger_at_node: Option<N>,
    pub trigger_action: Option<String>,
    pub trigger_actor: Value,
    pub trigger_claimed_by: Option<String>,
    /// Zaman alanı `Value` (bkz. `WfahPayload::Deadline`).
    pub trigger_claimed_at: Option<Value>,
}

/// Düşen bir kolun satırı — `_branch_cancelled` ve `_branch_superseded` ortak şekli.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BranchDropPayload<N> {
    /// `Ç3`: kolun KİMLİĞİ.
    pub branch_entry: N,
    /// `Ç3`: kolun düşme ANINDAKİ konumu.
    pub at_node: N,
    pub reason: String,
    /// `WOR-59`: iptal anındaki sahip. Yalnız `cancelled` yolunda dolu.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_at: Option<Value>,
    /// Yalnız `superseded` yolunda dolu — geçersizleşen onayın sahibi ve anı.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<Value>,
    pub trigger_kind: String,
    pub trigger_branch: Option<N>,
    pub trigger_action: Option<String>,
    pub trigger_actor: Value,
}

impl<N: Serialize> WfahPayload<N> {
    /// Satıra yazılacak JSON. `untagged` serileşme sayesinde **bugünkü şekiller
    /// birebir korunur** — hiçbir alan eklenmez.
    pub fn to_value(&self) -> Value {
        match self {
            WfahPayload::Join => Value::Null,
            other => serde_json::to_value(other).unwrap_or(Value::Null),
        }
    }
}

impl WfahPayload<String> {
    /// Ham satırdan payload'ı çözer. Ayrım satırın `action` ADINDAN gelen sınıftır —
    /// payload'ın içinde bir etiket ARANMAZ (bkz. modül başlığı).
    ///
    /// Tanınmayan ya da şekli tutmayan her şey `Unknown`a düşer: okuma TOPLAM
    /// fonksiyondur, hiçbir satır okuma yolunu patlatmaz.
    pub fn from_row(kind: WfahKind, input: Option<&Value>) -> Self {
        let Some(v) = input else {
            return match kind {
                WfahKind::Join => WfahPayload::Join,
                _ => WfahPayload::Unknown(Value::Null),
            };
        };
        let get = |k: &str| v.get(k).cloned();
        let s = |k: &str| v.get(k).and_then(|x| x.as_str().map(str::to_string));
        let nodes = |k: &str| {
            v.get(k)
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        match kind {
            WfahKind::Deadline => WfahPayload::Deadline {
                deadline: get("deadline"),
            },
            WfahKind::Escalation => match (s("after"), get("grant")) {
                (Some(after), Some(g)) => WfahPayload::Escalation {
                    after,
                    grant: GrantPayload {
                        c_a: g.get("c_a").cloned().unwrap_or(Value::Null),
                        when: g.get("when").and_then(|x| x.as_str().map(str::to_string)),
                    },
                },
                _ => WfahPayload::Unknown(v.clone()),
            },
            WfahKind::EscalationSkipped => match s("after") {
                Some(after) => WfahPayload::EscalationSkipped {
                    skipped: v.get("skipped").and_then(Value::as_bool).unwrap_or(true),
                    after,
                },
                None => WfahPayload::Unknown(v.clone()),
            },
            WfahKind::ClaimTaken => WfahPayload::ClaimTaken(v.clone()),
            WfahKind::ClaimReleased => WfahPayload::ClaimReleased(v.clone()),
            WfahKind::Trigger => WfahPayload::Trigger {
                result: get("result"),
                error: s("error"),
                message: s("message"),
                handled: v.get("handled").and_then(Value::as_bool),
                required: v.get("required").and_then(Value::as_bool),
            },
            WfahKind::CallReturn => WfahPayload::CallReturn {
                status: s("status"),
                callee_wfe_id: s("callee_wfe_id"),
            },
            WfahKind::CallTruncated => WfahPayload::CallTruncated {
                callee_wfe_id: s("callee_wfe_id"),
                omitted: v.get("omitted").and_then(Value::as_u64).unwrap_or(0) as usize,
                reason: s("reason").unwrap_or_default(),
            },
            WfahKind::Fork => WfahPayload::Fork {
                branches: nodes("branches"),
                join: get("join").unwrap_or(Value::Null),
                join_mode: s("join_mode").unwrap_or_default(),
                join_threshold: v
                    .get("join_threshold")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
                join_when: s("join_when"),
            },
            WfahKind::BranchArrived => match s("at_node") {
                Some(at_node) => WfahPayload::BranchArrived {
                    branch_entry: s("branch_entry"),
                    at_node,
                    approved_by: get("approved_by").unwrap_or(Value::Null),
                    approved_at: get("approved_at"),
                    claimed_at: get("claimed_at"),
                    claimed_by: s("claimed_by"),
                },
                None => WfahPayload::Unknown(v.clone()),
            },
            WfahKind::Join => WfahPayload::Join,
            WfahKind::BranchCancelled | WfahKind::BranchSuperseded => {
                match (s("branch_entry"), s("at_node"), s("reason")) {
                    (Some(branch_entry), Some(at_node), Some(reason)) => {
                        let drop = BranchDropPayload {
                            branch_entry,
                            at_node,
                            reason,
                            claimed_by: s("claimed_by"),
                            claimed_at: get("claimed_at").filter(|x| !x.is_null()),
                            approved_by: get("approved_by").filter(|x| !x.is_null()),
                            approved_at: get("approved_at").filter(|x| !x.is_null()),
                            trigger_kind: s("trigger_kind").unwrap_or_default(),
                            trigger_branch: s("trigger_branch"),
                            trigger_action: s("trigger_action"),
                            trigger_actor: get("trigger_actor").unwrap_or(Value::Null),
                        };
                        if kind == WfahKind::BranchCancelled {
                            WfahPayload::BranchCancelled(drop)
                        } else {
                            WfahPayload::BranchSuperseded(drop)
                        }
                    }
                    _ => WfahPayload::Unknown(v.clone()),
                }
            }
            WfahKind::Collapse => match (s("kind"), s("reason")) {
                (Some(k), Some(reason)) => WfahPayload::Collapse(CollapsePayload {
                    kind: k,
                    reason,
                    target: s("target"),
                    cancelled: nodes("cancelled"),
                    superseded: nodes("superseded"),
                    trigger_kind: s("trigger_kind").unwrap_or_default(),
                    trigger_branch: s("trigger_branch"),
                    trigger_at_node: s("trigger_at_node"),
                    trigger_action: s("trigger_action"),
                    trigger_actor: get("trigger_actor").unwrap_or(Value::Null),
                    trigger_claimed_by: s("trigger_claimed_by"),
                    trigger_claimed_at: get("trigger_claimed_at"),
                }),
                _ => WfahPayload::Unknown(v.clone()),
            },
            // Aksiyon satırı bu birleşimin KAPSAMI DIŞINDADIR (bkz. modül başlığı):
            // `input`u kullanıcının aksiyon girdisidir, marker payload'u değil.
            WfahKind::Action => WfahPayload::Unknown(v.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Şekil KORUNUR: `from_row` → `to_value` bugünkü payload'ı AYNEN geri vermeli.
    /// Bu, tipe geçişin saklanan satırları değiştirmediğinin kanıtıdır.
    fn round_trip(kind: WfahKind, raw: Value) {
        let p = WfahPayload::from_row(kind, Some(&raw));
        assert!(
            !matches!(p, WfahPayload::Unknown(_)),
            "{kind:?} şekli tanınmadı: {raw}"
        );
        assert_eq!(p.to_value(), raw, "{kind:?} şekli birebir korunmalı");
    }

    #[test]
    fn escalation_shape_is_preserved() {
        round_trip(
            WfahKind::Escalation,
            json!({"after": "P3D", "grant": {"c_a": {"c_orgu": "self"}, "when": "$ctx.x > 1"}}),
        );
        round_trip(
            WfahKind::Escalation,
            json!({"after": "P3D", "grant": {"c_a": {"c_orgu": "self"}}}),
        );
    }

    #[test]
    fn skipped_escalation_shape_is_preserved() {
        round_trip(
            WfahKind::EscalationSkipped,
            json!({"skipped": true, "after": "2026-09-08T10:00:00+00:00"}),
        );
    }

    #[test]
    fn trigger_has_three_shapes_in_one_variant() {
        round_trip(WfahKind::Trigger, json!({"result": {"skor": 750}}));
        round_trip(
            WfahKind::Trigger,
            json!({"error": "WFD.Timeout", "message": "zaman aşımı", "handled": true}),
        );
        round_trip(
            WfahKind::Trigger,
            json!({"error": "WFD.AutoexecFailed", "message": "hata", "handled": false, "required": false}),
        );
    }

    #[test]
    fn call_shapes_are_preserved() {
        round_trip(
            WfahKind::CallReturn,
            json!({"status": "completed", "callee_wfe_id": "abc"}),
        );
        round_trip(
            WfahKind::CallTruncated,
            json!({"callee_wfe_id": "abc", "omitted": 3, "reason": "call_history_truncated"}),
        );
    }

    #[test]
    fn deadline_shape_is_preserved() {
        round_trip(
            WfahKind::Deadline,
            json!({"deadline": "2026-09-08T10:00:00Z"}),
        );
    }

    /// ⚠️ `_collapse` payload'ının KENDİ `kind` alanı var — serde etiketi bu yüzden
    /// kullanılamadı. Alan aynen korunuyor.
    #[test]
    fn collapse_shape_keeps_its_own_kind_field() {
        let raw = json!({
            "kind": "collapse_to",
            "reason": "collapsed",
            "target": "self__coordinator",
            "cancelled": ["self__legalApprover"],
            "superseded": ["self__hrApprover"],
            "trigger_kind": "branch",
            "trigger_branch": "self__financeApprover",
            "trigger_at_node": "self__financeApprover",
            "trigger_action": "finans_ret",
            "trigger_actor": {"role": "financeApprover"},
            "trigger_claimed_by": null,
            "trigger_claimed_at": null
        });
        round_trip(WfahKind::Collapse, raw);
    }

    /// ⚠️ Kararın *"üçü aynı şekli taşır"* varsayımı diskte TUTMADI: düşen kolun
    /// satırı manşetin `cancelled`/`superseded` listelerini taşımaz, onun yerine
    /// kendi kimliğini/konumunu ve iptal ANINDAKİ claim'ini taşır.
    #[test]
    fn a_dropped_branch_row_has_its_own_shape() {
        round_trip(
            WfahKind::BranchCancelled,
            json!({
                "branch_entry": "self__legalApprover",
                "at_node": "self__legalApprover",
                "reason": "collapsed",
                "claimed_by": "u1",
                "claimed_at": "2026-09-08T10:00:00Z",
                "trigger_kind": "branch",
                "trigger_branch": "self__financeApprover",
                "trigger_action": "finans_ret",
                "trigger_actor": {"role": "financeApprover"}
            }),
        );
        // `superseded` ayrıca ONAYIN kimin olduğunu taşır — o onay artık sayılmıyor.
        round_trip(
            WfahKind::BranchSuperseded,
            json!({
                "branch_entry": "self__hrApprover",
                "at_node": "self__resultCoordinator",
                "reason": "join_quorum",
                "approved_by": {"role": "hrApprover"},
                "approved_at": "2026-09-08T10:00:00Z",
                "trigger_kind": "branch",
                "trigger_branch": "self__financeApprover",
                "trigger_action": "finans_onay",
                "trigger_actor": {"role": "financeApprover"}
            }),
        );
    }

    #[test]
    fn branch_arrived_shape_is_preserved() {
        round_trip(
            WfahKind::BranchArrived,
            json!({
                "branch_entry": "self__financeApprover",
                "at_node": "self__financeApprover",
                "approved_by": {"role": "financeApprover"},
                "approved_at": "2026-09-08T10:00:00Z",
                "claimed_at": null,
                "claimed_by": null
            }),
        );
    }

    /// Okuma TOPLAM fonksiyondur: tanınmayan şekil patlatmaz, `Unknown`a düşer.
    /// Bu geriye uyum kodu DEĞİL, deserialize'ın toplamlığıdır (Değişmez #9).
    #[test]
    fn an_unrecognised_shape_falls_to_unknown_instead_of_panicking() {
        let p = WfahPayload::from_row(WfahKind::Escalation, Some(&json!({"bilinmeyen": 1})));
        assert!(matches!(p, WfahPayload::Unknown(_)));
        let p = WfahPayload::from_row(WfahKind::Deadline, None);
        assert!(matches!(p, WfahPayload::Unknown(_)));
    }

    /// `_join` bugün payload TAŞIMAZ — `null` yazılır, `Unknown` DEĞİL.
    #[test]
    fn join_carries_no_payload() {
        let p = WfahPayload::from_row(WfahKind::Join, None);
        assert_eq!(p, WfahPayload::Join);
        assert_eq!(p.to_value(), Value::Null);
    }

    /// Aksiyon satırı KAPSAM DIŞI: girdisi marker payload'u değil, kullanıcının
    /// aksiyon girdisi. Sarmalansaydı `#.input.<yol>` sayımları bozulurdu.
    #[test]
    fn action_rows_stay_outside_the_union() {
        let p = WfahPayload::from_row(WfahKind::Action, Some(&json!({"tutar": 5})));
        assert_eq!(p, WfahPayload::Unknown(json!({"tutar": 5})));
        assert_eq!(
            p.to_value(),
            json!({"tutar": 5}),
            "ham girdi AYNEN geri verilmeli — sarmalama YOK"
        );
    }
}
