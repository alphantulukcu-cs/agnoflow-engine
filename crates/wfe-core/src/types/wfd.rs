use super::actor::CaRule;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Top-level WFD document — mirrors the JSON structure in CLAUDE.md exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WFD {
    #[serde(deserialize_with = "deserialize_stringish")]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(
        default = "default_wfd_version",
        deserialize_with = "deserialize_stringish"
    )]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    /// JSON Schema 2020-12 with x-visibility and x-wf-readonly extensions
    pub context: Value,
    pub start: Vec<StartRule>,
    pub actions: HashMap<String, ActionDef>,
    #[serde(default)]
    pub terminals: Vec<TerminalDef>,
    pub transitions: Vec<Transition>,
    #[serde(default)]
    pub listable: Vec<ListableRule>,
    pub terminal_when: String,
    #[serde(default, flatten)]
    pub extra: HashMap<String, Value>,
}

/// Named terminal node — referenced by ID from wft.conditions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalDef {
    pub id: String,
    #[serde(default)]
    pub wfes_effects: WfesEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wfe_end_response: Option<Value>,
}

/// Terminal reference in wft.conditions — either legacy `true`/`false` or a named ID string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TerminalRef {
    Bool(bool),
    Id(String),
}

impl Default for TerminalRef {
    fn default() -> Self {
        TerminalRef::Bool(false)
    }
}

impl TerminalRef {
    pub fn is_terminal(&self) -> bool {
        match self {
            TerminalRef::Bool(b) => *b,
            TerminalRef::Id(_) => true,
        }
    }
    pub fn id(&self) -> Option<&str> {
        match self {
            TerminalRef::Bool(_) => None,
            TerminalRef::Id(s) => Some(s.as_str()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionDef {
    pub name: String,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default = "true_expr")]
    pub when: String,
    pub c_a: Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    pub wft: WftRule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transition {
    pub id: String,
    pub when: String,
    /// Human transition: action name (references actions map key). None for autoexec transitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Autoexec transition: fires immediately when `when` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub autoexec: Option<AutoexecDef>,
    #[serde(default)]
    pub c_a: Vec<CaRule>,
    pub wfes_effects: WfesEffects,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<AutoexecDef>,
    pub wft: WftRule,
}

/// wft has two forms: simple (c_a array) or conditional (branching on ZEN).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WftRule {
    Simple {
        c_a: Vec<CaRule>,
    },
    Conditional {
        conditions: Vec<WftCondition>,
    },
    Parallel {
        parallel: Vec<WftParallelBranch>,
        join_when: ParallelJoin,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WftParallelBranch {
    #[serde(default)]
    pub c_a: Vec<CaRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParallelJoin {
    All,
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WftCondition {
    #[serde(default = "true_expr")]
    pub when: String,
    /// Legacy: `true`/`false`. New format: terminal ID string. Default false = not terminal.
    #[serde(default)]
    pub terminal: TerminalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wfe_end_response: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c_a: Option<Vec<CaRule>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<AutoexecDef>,
}

/// wfes_effects — {"set": {"field": value}} structure.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WfesEffects {
    #[serde(default)]
    pub set: HashMap<String, EffectValue>,
    #[serde(default)]
    pub append: HashMap<String, EffectValue>,
}

/// Values in wfes_effects.set — special strings ($actor etc.) or JSON refs or literals.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EffectValue {
    /// {"ref": "$ctx.field.path"} — dynamic reference into current DynCtx
    Ref {
        #[serde(rename = "ref")]
        path: String,
    },
    /// {"ctx": "field.path"} — editor shorthand for a DynCtx reference.
    Ctx { ctx: String },
    /// Any JSON literal, including special strings "$actor", "$timestamp", "$wfe_id",
    /// "$action.input.field_name"
    Literal(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListableRule {
    pub c_a: Vec<CaRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
}

/// Autoexec node definition — execution deferred to Plan 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoexecDef {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub params: Value,
}

fn default_wfd_version() -> String {
    "1.0.0".to_string()
}

fn true_expr() -> String {
    "true".to_string()
}

fn deserialize_stringish<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::String(s) => Ok(s),
        Value::Number(n) => Ok(n.to_string()),
        other => Err(serde::de::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
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

    #[test]
    fn editor_export_shape_deserializes() {
        let json = serde_json::json!({
            "id": "kredi-basvuru",
            "version": "1.0.0",
            "context": {
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "properties": {
                    "basvuran": {"type": "object", "x-wf-readonly": true}
                }
            },
            "start": [{
                "action": "Basvur",
                "when": "true",
                "c_a": [{"c_orgu": "*:[type:branch]", "c_r": ["memur"]}],
                "wfes_effects": {
                    "set": {"basvuran": "$actor"},
                    "append": {"notes": {"ctx": "previous_note"}}
                },
                "wft": {
                    "c_a": [{
                        "c_orgu": {"from": "basvuran.orgu", "traverse": "parent"},
                        "c_r": ["subeMuduru"]
                    }]
                }
            }],
            "actions": {
                "Basvur": {"name": "Basvur", "input": {"required": [], "optional": []}}
            },
            "transitions": [{
                "id": "terminal-flow",
                "when": "true",
                "action": "Onayla",
                "c_a": [{"c_orgu": "self", "c_r": ["subeMuduru"]}],
                "wfes_effects": {"set": {"status": "approved"}},
                "wft": {
                    "conditions": [{
                        "terminal": true,
                        "wfe_end_response": {"status": "approved"}
                    }]
                }
            }],
            "terminal_when": "false"
        });

        let wfd: WFD = serde_json::from_value(json).unwrap();
        assert_eq!(wfd.id, "kredi-basvuru");
        assert_eq!(wfd.name, "");
        assert_eq!(wfd.version, "1.0.0");
        assert_eq!(wfd.start[0].action.as_deref(), Some("Basvur"));
        assert_eq!(wfd.start[0].c_a[0].c_r, vec!["memur"]);
        assert!(matches!(
            wfd.start[0].wfes_effects.append["notes"],
            EffectValue::Ctx { .. }
        ));

        let WftRule::Conditional { conditions } = &wfd.transitions[0].wft else {
            panic!("expected conditional wft");
        };
        assert_eq!(conditions[0].when, "true");
    }
}
