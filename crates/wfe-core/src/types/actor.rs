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

/// A resolved (orgu, role) pair — one entry in the denormalized candidate cache
/// used by pool listings. c_u-only rules produce no entries here; claim
/// eligibility is always re-verified with the runtime matcher.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateActor {
    pub orgu_id: Uuid,
    pub role: String,
}
