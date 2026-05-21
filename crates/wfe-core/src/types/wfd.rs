use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;
use super::actor::CaRule;

/// Top-level WFD document — mirrors the JSON structure in CLAUDE.md exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WFD {
    pub id:            Uuid,
    pub name:          String,
    pub version:       u32,
    pub description:   Option<String>,
    /// JSON Schema 2020-12 with x-visibility and x-wf-readonly extensions
    pub context:       Value,
    pub start:         Vec<StartRule>,
    pub actions:       HashMap<String, ActionDef>,
    pub transitions:   Vec<Transition>,
    pub listable:      Vec<ListableRule>,
    pub terminal_when: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDef {
    pub name:        String,
    pub description: Option<String>,
    #[serde(default)]
    pub input: ActionInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActionInput {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartRule {
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    pub wft:          WftRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub id:           String,
    pub when:         String,
    pub action:       String,
    pub c_a:          Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger:      Option<AutoexecDef>,
    pub wft:          WftRule,
}

/// wft has two forms: simple (c_a array) or conditional (branching on ZEN).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WftRule {
    Simple { c_a: Vec<CaRule> },
    Conditional { conditions: Vec<WftCondition> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WftCondition {
    pub when:                        String,
    #[serde(default)]
    pub terminal:                    bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wfe_end_response:            Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c_a:                         Option<Vec<CaRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger:                     Option<AutoexecDef>,
}

/// wfes_effects — {"set": {"field": value}} structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WfesEffects {
    #[serde(default)]
    pub set: HashMap<String, EffectValue>,
}

/// Values in wfes_effects.set — special strings ($actor etc.) or JSON refs or literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectValue {
    /// {"ref": "$ctx.field.path"} — dynamic reference into current DynCtx
    Ref { #[serde(rename = "ref")] path: String },
    /// Any JSON literal, including special strings "$actor", "$timestamp", "$wfe_id",
    /// "$action.input.field_name"
    Literal(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListableRule {
    pub c_a:  Vec<CaRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Autoexec node definition — execution deferred to Plan 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoexecDef {
    #[serde(rename = "type")]
    pub kind:   String,
    #[serde(flatten)]
    pub params: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wfd_roundtrip() {
        let json = serde_json::json!({
            "id": "00000000-0000-0000-0000-000000000001",
            "name": "test-wfd",
            "version": 1,
            "context": {},
            "start": [{
                "c_a": [{"c_orgu": "self", "c_r": [["self", "clerk"]]}],
                "wfes_effects": {"set": {"status": "pending"}},
                "wft": {"c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}]}
            }],
            "actions": {
                "approve": {"name": "approve", "input": {"required": [], "optional": []}}
            },
            "transitions": [{
                "id": "t1",
                "when": "$status == 'pending'",
                "action": "approve",
                "c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}],
                "wfes_effects": {"set": {"status": "approved"}},
                "wft": {"c_a": [{"c_orgu": "self", "c_r": [["self", "manager"]]}]}
            }],
            "listable": [],
            "terminal_when": "$status == 'approved'"
        });

        let wfd: WFD = serde_json::from_value(json.clone()).expect("deserialize");
        assert_eq!(wfd.name, "test-wfd");
        assert_eq!(wfd.transitions.len(), 1);
        assert_eq!(wfd.transitions[0].id, "t1");

        assert!(matches!(wfd.transitions[0].wft, WftRule::Simple { .. }));

        let back: WFD = serde_json::from_str(&serde_json::to_string(&wfd).unwrap()).unwrap();
        assert_eq!(back.name, wfd.name);
    }

    #[test]
    fn wft_conditional_deserializes() {
        let json = serde_json::json!({
            "conditions": [
                {"when": "$amount < 1000", "terminal": true,
                 "wfe_end_response": {"status": "approved"}},
                {"when": "$amount >= 1000", "c_a": [{"c_orgu": "self", "c_r": [["self", "director"]]}]}
            ]
        });
        let wft: WftRule = serde_json::from_value(json).unwrap();
        assert!(matches!(wft, WftRule::Conditional { .. }));
    }
}
