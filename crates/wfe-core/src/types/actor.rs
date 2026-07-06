use serde::Deserializer;
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

/// A resolved (orgu, role) pair — one entry in the candidate actor set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateActor {
    pub orgu_id: Uuid,
    pub role: String,
}

/// One rule in a c_a array (OR across rules, AND within a rule).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaRule {
    pub c_orgu: COrguExpr,
    #[serde(default, deserialize_with = "deserialize_roles")]
    pub c_r: Vec<String>,
    #[serde(default)]
    pub c_u: Vec<String>,
}

/// WFAH query anchor: look up the last WFAH entry by action name and extract actor.orgu.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WfahQuery {
    pub wfah:  String,
    pub field: String,
}

/// The `from` field in an anchored c_orgu: either a DynCtx path string or a WFAH query object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum COrguFrom {
    Wfah(WfahQuery),
    DynCtx(String),
}

/// Three forms of c_orgu as defined in CLAUDE.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum COrguExpr {
    /// {"from": {"wfah": "...", "field": "actor.orgu"}, "traverse": "..."}
    /// {"from": "$ctx.field.orgu", "traverse": "..."}
    Anchored { from: COrguFrom, traverse: String },
    /// ORGTRVLANG expr string or "*:[type:branch]"
    Expr(String),
}

fn deserialize_roles<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let arr = value
        .as_array()
        .ok_or_else(|| serde::de::Error::custom("c_r must be an array"))?;

    arr.iter()
        .map(|item| {
            if let Some(role) = item.as_str() {
                return Ok(role.to_string());
            }
            if let Some(pair) = item.as_array() {
                if pair.len() == 2 {
                    if let Some(role) = pair[1].as_str() {
                        return Ok(role.to_string());
                    }
                }
            }
            Err(serde::de::Error::custom(
                "c_r entries must be role strings or [scope, role] pairs",
            ))
        })
        .collect()
}
