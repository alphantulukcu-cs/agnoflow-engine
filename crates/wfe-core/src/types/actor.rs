use crate::v22::ownership::ClaimAuthority;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Exact (ORGU, (U, R)) triple — the only valid actor representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub orgu_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
}

/// Minimal org unit returned by OrgPort.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgUnit {
    pub orgu_id: Uuid,
    pub orgu_type: serde_json::Value,
    pub path: String,
}

/// A resolved (orgu, role) or (orgu, user) entry — one item in the denormalized
/// candidate cache used by pool listings (WOR-44). Role entries carry `role`
/// and leave `user_id`/`user_ident` unset; c_u entries carry an empty `role`
/// and either `user_id` (c_u parsed as a UUID) or `user_ident` (c_u is a
/// non-UUID identifier, mirrored from matcher.rs's identity channel). Claim
/// eligibility is always re-verified with the runtime matcher — this cache is
/// only for over-inclusive VIEW visibility.
///
/// **Anchorless entries** (`c_orgu` absent in the rule) carry `any_orgu: true` and NO
/// `orgu_id`: the person matches from any unit, so materialising one row per tenant ORGU
/// would be both wrong (the set changes as the org tree changes) and unbounded. Pool
/// listing has a dedicated containment filter for these (`portal/pool.rs`). The marker is
/// explicit rather than "orgu_id missing" so that `@> [{"user_id": U}]` can never
/// accidentally match a SCOPED entry for the same person in a different unit.
/// `PartialEq`: `view_grants` aynı adayı iki kuraldan (listable + wf_admin)
/// üretebiliyor, projeksiyon kolonuna tekrar yazmamak için karşılaştırılır.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateActor {
    /// `None` yalnız `any_orgu = true` girdilerde — çapalı girdilerde daima yazılır
    /// (eski satırların JSON'u birebir aynı kalır).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub orgu_id: Option<Uuid>,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_ident: Option<String>,
    /// Çapasız kural girdisi: birim kısıtı YOK. `false` iken serileştirilmez — mevcut
    /// `current_c_a` satırlarının biçimi değişmez.
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub any_orgu: bool,
    /// `R03`/S1-c — adayı HANGİ KURALIN uygun kıldığı. `E12`'nin enum'u AYNEN
    /// (`c_a | grant`); ikinci bir sözcük (`source`, `via_grant`) açılmadı çünkü
    /// soru defterdekiyle aynıdır, yalnız öznesi tekil owner değil aday kümesidir.
    ///
    /// **YALNIZ act kolonlarında yazılır** (`wf.wfe.current_c_a`,
    /// `wf.wfe_branch.c_a`). Görünürlük kolonlarında (`view_c_a`,
    /// `current_view_c_a`, `end_view_c_a`, `wfe_branch.view_c_a`) escalation grant'ı
    /// kavramı YOKTUR — oradaki genişleme `listable`/`wf_admin` eksenidir ve bu alan
    /// o ekseni adlandırmıyor. Ayrımı kolon kimliği taşır.
    ///
    /// **Öncelik `E12`'den AYNEN: `c_a` kazanır.** Aynı aday hem tabandan hem açık bir
    /// grant'tan doğuyorsa (`R05` gölgelemesi meşrudur) satır `c_a` sayılır; böylece
    /// `unit-workload`un iki sayacı (`active` / `also_eligible`) AYRIK kalır.
    ///
    /// `None` iki şey demektir: ya bu bir act adayı değildir, ya da satır alan
    /// eklenmeden önce yazılmıştır. İkincisi rapora `active` olarak girer — backfill
    /// YAZILMAZ (`R01`), kolon canlı durumdur ve bir sonraki commit'te yeniden yazılır.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authority: Option<ClaimAuthority>,
}

impl CandidateActor {
    /// İki adayın AYNI KİŞİ/KÜME'yi gösterip göstermediği — kaynak damgası HARİÇ.
    ///
    /// `node_candidates` gölgelenen adayı (`R05`) tek satırda tutmak için buna sorar;
    /// türetilmiş `PartialEq` kullanılamaz, çünkü `authority` ona dahildir ve aynı
    /// aday "taban" ile "grant" damgalarıyla İKİ satır olurdu — rapor o işi o birimde
    /// hem `active` hem `also_eligible` sayardı.
    ///
    /// Gövde alanları TEK TEK açıyor (joker yok): kimliğe yeni bir alan eklendiğinde
    /// derleyici burayı işaret eder ve "eşitliğe girer mi" sorusu cevapsız geçemez.
    pub fn same_actor(&self, other: &Self) -> bool {
        let Self {
            orgu_id,
            role,
            user_id,
            user_ident,
            any_orgu,
            authority: _,
        } = self;
        *orgu_id == other.orgu_id
            && *role == other.role
            && *user_id == other.user_id
            && *user_ident == other.user_ident
            && *any_orgu == other.any_orgu
    }
}
