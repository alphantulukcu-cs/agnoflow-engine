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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateActor {
    pub orgu_id: Uuid,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub user_ident: Option<String>,
}
