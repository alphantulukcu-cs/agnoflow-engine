use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Immutable DynCtx snapshot. merge() always returns a new instance.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DynCtx(pub Value);

impl DynCtx {
    pub fn empty() -> Self {
        Self(Value::Object(serde_json::Map::new()))
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Merges a flat map of key→value into a new DynCtx. Never mutates self.
    pub fn merge(&self, patch: serde_json::Map<String, Value>) -> Self {
        let mut map = match &self.0 {
            Value::Object(m) => m.clone(),
            _ => serde_json::Map::new(),
        };
        for (k, v) in patch {
            map.insert(k, v);
        }
        Self(Value::Object(map))
    }
}
