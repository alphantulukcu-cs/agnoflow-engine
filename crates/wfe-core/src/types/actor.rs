use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Exact (ORGU, (U, R)) triple — the only valid actor representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Actor {
    pub orgu_id: Uuid,
    pub user_id: Uuid,
    pub role:    String,
}

/// Minimal org unit returned by OrgPort.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgUnit {
    pub orgu_id:   Uuid,
    pub orgu_type: serde_json::Value,
    pub path:      String,
}

/// A resolved (orgu, role) pair — one entry in the candidate actor set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateActor {
    pub orgu_id: Uuid,
    pub role:    String,
}

/// One rule in a c_a array (OR across rules, AND within a rule).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaRule {
    pub c_orgu: COrguExpr,
    #[serde(default)]
    pub c_r:    Vec<[String; 2]>,
    #[serde(default)]
    pub c_u:    Vec<String>,
}

/// Two forms of c_orgu as defined in CLAUDE.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum COrguExpr {
    /// {"from": "$ctx.field.orgu", "traverse": "self"}
    Anchored { from: String, traverse: String },
    /// ORGTRVLANG expr string or "*:[type:branch]"
    Expr(String),
}
