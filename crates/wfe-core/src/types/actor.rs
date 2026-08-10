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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}
