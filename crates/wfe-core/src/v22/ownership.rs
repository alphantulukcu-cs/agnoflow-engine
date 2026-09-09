//! Sahiplik marker ailesi — `claim_taken:` / `claim_released:` (Ç13, Ç1-EK, E12).
//!
//! # Neden tek modül
//!
//! Sahipliği değiştiren BEŞ kapı var (kendi alma, vekaleten alma, yetkili devir,
//! WF Admin müdahalesi, SLA-1/grant guard bırakması) ve hepsi AYNI iki satır
//! şeklini yazar. Şekil kapıların içine dağıtılsaydı "bir satır = bir sahiplik
//! öznesi" değişmezini her kapı ayrı ayrı korumak zorunda kalırdı — Ç13'ün
//! kapatmak için yola çıktığı *"yokluğu anlam taşır"* antipattern'i tam olarak
//! böyle doğmuştu (`reassign` satırında yol adının YOKLUĞU "node kuralı" demekti).
//!
//! # Kapalı listeler
//!
//! `via` (4), `authority` (2) ve `reason` (5) motorda **enum**dur, düz metin
//! DEĞİL (E12/S1): yeni bir sahiplenme/bırakma kapısı açan geliştirici varyant
//! eklemeden DERLEYEMEZ. Serileşmiş adlar `P04`ün tip yüzeyi ve tasarımcının
//! `count($wfah, #.input.via == "…")` sayımlarıdır — **sözleşmedir**.
//!
//! Koşullu alanlar (`delegation`, `global_action`, `after`) enum varyantına
//! BAĞLIDIR ve yapıcıdan gelir: `ClaimTaken::delegated` delegasyonsuz, `::timeout`
//! süresiz KURULAMAZ. Alanlar açık olsaydı `via: "delegated"` yazıp delegasyonu
//! unutan bir kapı sessizce derlenirdi.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::types::wfah::Wfah;
use crate::types::wfd_v22::GlobalAction;
use crate::v22::wfah_kind::{parse_marker, WfahKind};

/// Sahipliğin NASIL doğduğu (E12/S1, Ç13'ten aynen 4 değer).
///
/// `authority` ile ÇARPILIR, karıştırılmaz: grant'la havuza giren kişi de işi
/// kendi alabilir (`self`), vekaleten alabilir (`delegated`) ya da kendisine
/// atanabilir (`assigned` / `admin_assigned`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimVia {
    /// Kişi işi kendi aldı.
    #[serde(rename = "self")]
    SelfClaim,
    /// Kişi bir başkasına vekâleten aldı (delegasyon kaydı payload'da).
    Delegated,
    /// Node'un kendi `reassign` kuralına uyan bir yetkili atadı.
    Assigned,
    /// WF Admin bir global aksiyonla atadı (aksiyon payload'da).
    AdminAssigned,
}

/// Owner'ı HANGİ KURALIN uygun kıldığı (E12/S1) — YALNIZ `claim_taken:` satırında.
///
/// **Öncelik: `c_a` KAZANIR.** Aktör hem node'un taban `c_a`'sına hem açık bir
/// grant'a uyuyorsa `c_a` yazılır; `grant` değerinin anlamı *"o grant açılmasaydı
/// bu kişi bu işi ALAMAZDI"*tır. K10'un havuz genişletme kazancının tek sorguluk
/// kanıtı budur.
/// `R03`/S1-c: aynı enum ÇÖZÜLMÜŞ ADAY kaydında da yaşıyor
/// (`types::actor::CandidateActor::authority`) ve o kayıt `current_c_a`dan geri
/// OKUNUYOR — `Deserialize` bu yüzden burada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimAuthority {
    /// Node'un taban `c_a` kuralı yetti.
    #[serde(rename = "c_a")]
    Ca,
    /// Taban kural yetmedi, açık bir grant yetkilendirdi.
    Grant,
}

/// Sahipliğin NEDEN düştüğü (E12/S1, Ç13'ün 5 değeri; `timeout` Ç1-EK'ten,
/// `grant_guard_false` Ç9'dan).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimReason {
    /// Sahip işi kendi bıraktı (ya da kendi devretti).
    #[serde(rename = "self")]
    SelfRelease,
    /// Node'un `reassign` kuralına uyan bir yetkili işi sahibinden aldı.
    TakenByOther,
    /// WF Admin bir global aksiyonla aldı (aksiyon payload'da).
    Admin,
    /// SLA-1 süresi doldu (`after` payload'da).
    Timeout,
    /// Grant `when` guard'ı false'a döndü (Ç9).
    GrantGuardFalse,
}

/// Vekaleten claim'in delegasyon kaydı — `via: "delegated"` satırının ZORUNLU eki.
#[derive(Debug, Clone)]
pub struct ClaimDelegation {
    pub delegation_id: Uuid,
    pub delegator: Uuid,
    pub seat_orgu_id: Uuid,
    pub seat_role: String,
}

/// Paralel modda sahiplik satırının kol alanları (E12/S5) — **birlikte ZORUNLU**.
///
/// Tek yapı olmasının sebebi E14: kol satırları tur kapanınca SİLİNİR, yani
/// alanlar yazma anında dolmazsa geri türetilemez. İkisi ayrı `Option` olsaydı
/// birini yazıp diğerini unutan kapı derlenirdi.
#[derive(Debug, Clone, Copy)]
pub struct OwnershipBranch<'a> {
    /// Kolun DEĞİŞMEZ kimliği (`BranchState::entry_node`, Ç3).
    pub entry: &'a str,
    /// Kolun o ANKİ konumu (Ç3).
    pub at_node: &'a str,
}

/// `claim_taken:<node_key>` satırının adı + payload'ı.
#[derive(Debug, Clone)]
pub struct ClaimTaken<'a> {
    node_key: &'a str,
    owner: Uuid,
    via: ClaimVia,
    authority: ClaimAuthority,
    /// E12/S2. `None` = taban BİLİNMİYOR (defterde `to_node` taşıyan satır yok,
    /// R01'in mirası) → alan YAZILMAZ. Yedek yol kurmak (0 yazmak) bekleme
    /// süresini sıfır diye YALAN söylerdi; R02/S2 aynı gerekçeyle fallback yasakladı.
    waited_for_seconds: Option<i64>,
    delegation: Option<ClaimDelegation>,
    global_action: Option<GlobalAction>,
    branch: Option<OwnershipBranch<'a>>,
}

impl<'a> ClaimTaken<'a> {
    fn new(node_key: &'a str, owner: Uuid, via: ClaimVia, authority: ClaimAuthority) -> Self {
        Self {
            node_key,
            owner,
            via,
            authority,
            waited_for_seconds: None,
            delegation: None,
            global_action: None,
            branch: None,
        }
    }

    /// Kişi işi kendi aldı.
    pub fn by_self(node_key: &'a str, owner: Uuid, authority: ClaimAuthority) -> Self {
        Self::new(node_key, owner, ClaimVia::SelfClaim, authority)
    }

    /// Kişi işi vekâleten aldı — delegasyon kaydı ZORUNLU.
    pub fn delegated(
        node_key: &'a str,
        owner: Uuid,
        authority: ClaimAuthority,
        delegation: ClaimDelegation,
    ) -> Self {
        Self {
            delegation: Some(delegation),
            ..Self::new(node_key, owner, ClaimVia::Delegated, authority)
        }
    }

    /// Node'un `reassign` kuralına uyan yetkili atadı.
    pub fn assigned(node_key: &'a str, owner: Uuid, authority: ClaimAuthority) -> Self {
        Self::new(node_key, owner, ClaimVia::Assigned, authority)
    }

    /// WF Admin atadı — geçilen global aksiyon ZORUNLU (Ç13: üç global aksiyonu
    /// `action` adı DEĞİL bu alan ayırır).
    pub fn admin_assigned(
        node_key: &'a str,
        owner: Uuid,
        authority: ClaimAuthority,
        global_action: GlobalAction,
    ) -> Self {
        Self {
            global_action: Some(global_action),
            ..Self::new(node_key, owner, ClaimVia::AdminAssigned, authority)
        }
    }

    /// E12/S2: bekleme süresi. Yetkili devirde (kaybeden bir sahip VARSA) **0**.
    pub fn waited(mut self, seconds: Option<i64>) -> Self {
        self.waited_for_seconds = seconds;
        self
    }

    /// Paralel mod kol alanları (E12/S5); tekil modda çağrılmaz.
    pub fn in_branch(mut self, branch: Option<OwnershipBranch<'a>>) -> Self {
        self.branch = branch;
        self
    }

    pub fn marker(&self) -> String {
        format!("claim_taken:{}", self.node_key)
    }

    pub fn input(&self) -> Value {
        let mut input = json!({
            "via": self.via,
            "authority": self.authority,
            "owner": self.owner.to_string(),
        });
        if let Some(d) = &self.delegation {
            input["delegation"] = json!({
                "delegation_id": d.delegation_id.to_string(),
                "delegator": d.delegator.to_string(),
                "seat": { "orgu_id": d.seat_orgu_id.to_string(), "role": d.seat_role },
            });
        }
        if let Some(ga) = self.global_action {
            input["global_action"] = json!(ga.as_str());
        }
        if let Some(w) = self.waited_for_seconds {
            input["waited_for_seconds"] = json!(w);
        }
        put_branch(&mut input, self.branch);
        input
    }
}

/// `claim_released:<node_key>` satırının adı + payload'ı.
#[derive(Debug, Clone)]
pub struct ClaimReleased<'a> {
    node_key: &'a str,
    owner: Uuid,
    reason: ClaimReason,
    /// `None` = sahiplik başlangıcı bilinmiyor (eski satır) → alan YAZILMAZ.
    held_for_seconds: Option<i64>,
    after: Option<&'a str>,
    global_action: Option<GlobalAction>,
    branch: Option<OwnershipBranch<'a>>,
}

impl<'a> ClaimReleased<'a> {
    fn new(node_key: &'a str, owner: Uuid, reason: ClaimReason) -> Self {
        Self {
            node_key,
            owner,
            reason,
            held_for_seconds: None,
            after: None,
            global_action: None,
            branch: None,
        }
    }

    /// Sahip işi kendi bıraktı ya da kendi devretti.
    pub fn by_self(node_key: &'a str, owner: Uuid) -> Self {
        Self::new(node_key, owner, ClaimReason::SelfRelease)
    }

    /// Yetkili bir başkası işi sahibinden aldı.
    pub fn taken_by_other(node_key: &'a str, owner: Uuid) -> Self {
        Self::new(node_key, owner, ClaimReason::TakenByOther)
    }

    /// WF Admin aldı — geçilen global aksiyon ZORUNLU.
    pub fn by_admin(node_key: &'a str, owner: Uuid, global_action: GlobalAction) -> Self {
        Self {
            global_action: Some(global_action),
            ..Self::new(node_key, owner, ClaimReason::Admin)
        }
    }

    /// SLA-1 süresi doldu — süre ZORUNLU (Ç1-EK: `after` YALNIZ bu satırda).
    pub fn timeout(node_key: &'a str, owner: Uuid, after: &'a str) -> Self {
        Self {
            after: Some(after),
            ..Self::new(node_key, owner, ClaimReason::Timeout)
        }
    }

    /// Grant `when` guard'ı false'a döndü (Ç9) — `after` YAZILMAZ.
    pub fn grant_guard_false(node_key: &'a str, owner: Uuid) -> Self {
        Self::new(node_key, owner, ClaimReason::GrantGuardFalse)
    }

    /// Sahipliğin ne kadar tutulduğu (Ç1: tam sayı saniye).
    pub fn held(mut self, seconds: Option<i64>) -> Self {
        self.held_for_seconds = seconds;
        self
    }

    /// Paralel mod kol alanları (E12/S5); tekil modda çağrılmaz.
    pub fn in_branch(mut self, branch: Option<OwnershipBranch<'a>>) -> Self {
        self.branch = branch;
        self
    }

    pub fn marker(&self) -> String {
        format!("claim_released:{}", self.node_key)
    }

    pub fn input(&self) -> Value {
        let mut input = json!({
            "reason": self.reason,
            "owner": self.owner.to_string(),
        });
        if let Some(after) = self.after {
            input["after"] = json!(after);
        }
        if let Some(ga) = self.global_action {
            input["global_action"] = json!(ga.as_str());
        }
        if let Some(h) = self.held_for_seconds {
            input["held_for_seconds"] = json!(h);
        }
        put_branch(&mut input, self.branch);
        input
    }
}

fn put_branch(input: &mut Value, branch: Option<OwnershipBranch<'_>>) {
    if let Some(b) = branch {
        input["branch_entry"] = json!(b.entry);
        input["at_node"] = json!(b.at_node);
    }
}

/// E12/S2 — `waited_for_seconds`ın TABANI: *node/kol girişi* VEYA *aynı node/koldaki
/// son bırakma anı*, **hangisi daha YENİ ise**.
///
/// Node girişi ikinci kez TANIMLANMAZ: tekil modda `pipeline::node_entered_at`
/// (R02'nin `to_node != null` tanımı), paralel modda `BranchState::entered_at`
/// gerçek kolonu — ikisi de çağırandan gelir.
///
/// Son bırakma anı olmadan hesap YANLIŞ olurdu: al-bırak-al döngüsünde ikinci
/// sahibin beklemesi birincinin `held_for_seconds`ıyla ÇAKIŞIRDI (aynı saniyeler
/// iki alanda iki kez sayılır).
pub fn wait_base(
    wfah: &Wfah,
    node_key: &str,
    branch_entry: Option<&str>,
    entered_at: DateTime<Utc>,
) -> DateTime<Utc> {
    let last_release = wfah
        .entries()
        .iter()
        .rev()
        .find(|e| {
            if e.branch_entry.as_deref() != branch_entry {
                return false;
            }
            let parsed = parse_marker(&e.action);
            parsed.kind == WfahKind::ClaimReleased && parsed.node.as_deref() == Some(node_key)
        })
        .map(|e| e.applied_at);
    match last_release {
        Some(released_at) if released_at > entered_at => released_at,
        _ => entered_at,
    }
}

/// İki an arası TAM SAYI saniye (Ç1 deseni). Negatif fark 0'a kırpılır: saat
/// kayması bir bekleme süresini eksi gösteremez.
pub fn seconds_between(from: DateTime<Utc>, to: DateTime<Utc>) -> i64 {
    (to - from).num_seconds().max(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::actor::Actor;
    use crate::types::wfah::WfahEntry;
    use chrono::Duration;

    fn actor() -> Actor {
        Actor {
            user_id: Uuid::nil(),
            orgu_id: Uuid::nil(),
            role: "memur".into(),
        }
    }

    fn entry(seq: u32, action: &str, at: DateTime<Utc>, branch: Option<&str>) -> WfahEntry {
        WfahEntry {
            seq,
            action: action.into(),
            actor: actor(),
            input: None,
            applied_at: at,
            from_node: None,
            to_node: None,
            branch_entry: branch.map(str::to_string),
            branch_round: branch.map(|_| 1),
        }
    }

    /// Serileşmiş adlar `#.input.via` / `.authority` / `.reason` DEĞER KÜMESİDİR.
    #[test]
    fn closed_lists_serialise_to_the_contract_names() {
        let via = [
            (ClaimVia::SelfClaim, "self"),
            (ClaimVia::Delegated, "delegated"),
            (ClaimVia::Assigned, "assigned"),
            (ClaimVia::AdminAssigned, "admin_assigned"),
        ];
        assert_eq!(via.len(), 4, "via kapalı listesi 4 değerdir (E12/S1)");
        for (v, name) in via {
            assert_eq!(serde_json::to_value(v).unwrap(), json!(name));
        }
        let authority = [(ClaimAuthority::Ca, "c_a"), (ClaimAuthority::Grant, "grant")];
        assert_eq!(authority.len(), 2, "authority kapalı listesi 2 değerdir");
        for (a, name) in authority {
            assert_eq!(serde_json::to_value(a).unwrap(), json!(name));
        }
        let reason = [
            (ClaimReason::SelfRelease, "self"),
            (ClaimReason::TakenByOther, "taken_by_other"),
            (ClaimReason::Admin, "admin"),
            (ClaimReason::Timeout, "timeout"),
            (ClaimReason::GrantGuardFalse, "grant_guard_false"),
        ];
        assert_eq!(reason.len(), 5, "reason kapalı listesi 5 değerdir");
        for (r, name) in reason {
            assert_eq!(serde_json::to_value(r).unwrap(), json!(name));
        }
    }

    #[test]
    fn taken_marker_carries_node_key_in_its_name() {
        let owner = Uuid::new_v4();
        let taken = ClaimTaken::by_self("self__memur", owner, ClaimAuthority::Ca).waited(Some(12));
        assert_eq!(taken.marker(), "claim_taken:self__memur");
        let input = taken.input();
        assert_eq!(input["via"], json!("self"));
        assert_eq!(input["authority"], json!("c_a"));
        assert_eq!(input["owner"], json!(owner.to_string()));
        assert_eq!(input["waited_for_seconds"], json!(12));
        assert!(input.get("delegation").is_none());
        assert!(input.get("branch_entry").is_none());
    }

    /// `authority` YALNIZ `claim_taken:`te (E12/S1) — bırakmada uygunluk sorusu yok.
    #[test]
    fn released_never_carries_authority() {
        let owner = Uuid::new_v4();
        let released = ClaimReleased::timeout("self__memur", owner, "PT2H").held(Some(7200));
        assert_eq!(released.marker(), "claim_released:self__memur");
        let input = released.input();
        assert!(input.get("authority").is_none());
        assert_eq!(input["reason"], json!("timeout"));
        assert_eq!(input["after"], json!("PT2H"));
        assert_eq!(input["held_for_seconds"], json!(7200));
        assert_eq!(input["owner"], json!(owner.to_string()));
        // `global_action` YALNIZ reason=admin satırında.
        assert!(input.get("global_action").is_none());
    }

    #[test]
    fn branch_fields_are_written_as_a_pair() {
        let input = ClaimTaken::assigned("self__b", Uuid::new_v4(), ClaimAuthority::Grant)
            .in_branch(Some(OwnershipBranch {
                entry: "self__fork_a",
                at_node: "self__b",
            }))
            .input();
        assert_eq!(input["branch_entry"], json!("self__fork_a"));
        assert_eq!(input["at_node"], json!("self__b"));
        assert_eq!(input["authority"], json!("grant"));
    }

    #[test]
    fn admin_paths_carry_the_global_action() {
        let taken = ClaimTaken::admin_assigned(
            "self__memur",
            Uuid::new_v4(),
            ClaimAuthority::Ca,
            GlobalAction::AssignFromPool,
        )
        .input();
        assert_eq!(taken["global_action"], json!("assign_from_pool"));
        assert_eq!(taken["via"], json!("admin_assigned"));
        let released =
            ClaimReleased::by_admin("self__memur", Uuid::new_v4(), GlobalAction::ReclaimToPool)
                .input();
        assert_eq!(released["global_action"], json!("reclaim_to_pool"));
        assert_eq!(released["reason"], json!("admin"));
        assert!(released.get("after").is_none());
    }

    /// Taban al-bırak-al döngüsünde İKİNCİ almaya kayar; aksi halde ikinci sahibin
    /// beklemesi birincinin tutma süresini de sayardı.
    #[test]
    fn wait_base_prefers_the_later_of_entry_and_last_release() {
        let entered = Utc::now() - Duration::hours(3);
        let released_at = entered + Duration::hours(1);
        let wfah = Wfah(vec![
            entry(1, "basvuru", entered, None),
            entry(2, "claim_released:self__memur", released_at, None),
        ]);
        assert_eq!(
            wait_base(&wfah, "self__memur", None, entered),
            released_at,
            "son bırakma girişten YENİ ise taban odur"
        );
        // Başka bir node'un bırakması tabanı KAYDIRMAZ.
        assert_eq!(wait_base(&wfah, "self__amir", None, entered), entered);
    }

    /// Kol alanı eşleşmezse satır başka bir kolun geçmişidir — taban kaymaz.
    #[test]
    fn wait_base_is_branch_scoped() {
        let entered = Utc::now() - Duration::hours(2);
        let released_at = entered + Duration::minutes(30);
        let wfah = Wfah(vec![entry(
            1,
            "claim_released:self__b",
            released_at,
            Some("self__fork_a"),
        )]);
        assert_eq!(
            wait_base(&wfah, "self__b", Some("self__fork_a"), entered),
            released_at
        );
        assert_eq!(wait_base(&wfah, "self__b", Some("self__fork_z"), entered), entered);
        assert_eq!(wait_base(&wfah, "self__b", None, entered), entered);
    }

    #[test]
    fn seconds_are_whole_and_never_negative() {
        let now = Utc::now();
        assert_eq!(seconds_between(now - Duration::seconds(90), now), 90);
        assert_eq!(seconds_between(now + Duration::seconds(5), now), 0);
    }
}
