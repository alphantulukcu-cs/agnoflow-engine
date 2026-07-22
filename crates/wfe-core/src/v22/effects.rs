//! M9/WOR-42 — wfes_effects uygulama + $-string çözümleme.
//! v2.2'de {ref}/{ctx} obje formları ve `_step_` injection KALDIRILDI;
//! effect değerleri düz JSON'dur, $-önekli string'ler çözülür:
//! `$actor`, `$timestamp`, `$wfe_id`, `$node`, `$ctx.<path>`,
//! `$action.input.<path>`, `$exec.result.<path>`.

use crate::error::EngineError;
use crate::types::actor::Actor;
use crate::types::wfd_v22::WfesEffects;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use uuid::Uuid;

#[derive(Clone)]
pub struct EffectEnv<'a> {
    pub actor: &'a Actor,
    pub wfe_id: Uuid,
    pub node: Option<&'a str>,
    pub action_input: Option<&'a Value>,
    pub exec_result: Option<&'a Value>,
    pub now: DateTime<Utc>,
}

/// Effects'i staged ctx üzerine uygular; yeni bir ctx döner (immutable).
/// Set path'leri dotted olabilir — ara objeler oluşturulur.
pub fn apply_effects(
    ctx: &Value,
    effects: &WfesEffects,
    env: &EffectEnv<'_>,
) -> Result<Value, EngineError> {
    let mut root = match ctx {
        Value::Object(m) => m.clone(),
        _ => Map::new(),
    };
    for (path, raw) in &effects.set {
        let resolved = resolve_value(raw, ctx, env)?;
        set_path(&mut root, path, resolved);
    }
    Ok(Value::Object(root))
}

/// Bir effect/terminal değerini çözer. String'ler $-kurallarına göre,
/// obje/array'ler recursive işlenir, diğerleri literal kalır.
pub fn resolve_value(raw: &Value, ctx: &Value, env: &EffectEnv<'_>) -> Result<Value, EngineError> {
    match raw {
        Value::String(s) => resolve_dollar_string(s, ctx, env),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), resolve_value(v, ctx, env)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(arr) => Ok(Value::Array(
            arr.iter()
                .map(|v| resolve_value(v, ctx, env))
                .collect::<Result<_, _>>()?,
        )),
        other => Ok(other.clone()),
    }
}

fn resolve_dollar_string(s: &str, ctx: &Value, env: &EffectEnv<'_>) -> Result<Value, EngineError> {
    match s {
        "$actor" => serde_json::to_value(env.actor)
            .map_err(|e| EngineError::EffectValue(format!("$actor serileştirilemedi: {e}"))),
        "$timestamp" => Ok(Value::from(env.now.to_rfc3339())),
        "$wfe_id" => Ok(Value::from(env.wfe_id.to_string())),
        "$node" => Ok(env.node.map(Value::from).unwrap_or(Value::Null)),
        _ => {
            if let Some(path) = s.strip_prefix("$ctx.") {
                return Ok(get_path(ctx, path).cloned().unwrap_or(Value::Null));
            }
            if let Some(path) = s.strip_prefix("$action.input.") {
                let input = env.action_input.unwrap_or(&Value::Null);
                return Ok(get_path(input, path).cloned().unwrap_or(Value::Null));
            }
            if let Some(path) = s.strip_prefix("$exec.result.") {
                let result = env.exec_result.unwrap_or(&Value::Null);
                return Ok(get_path(result, path).cloned().unwrap_or(Value::Null));
            }
            if s.starts_with("$exec.response.") {
                return Err(EngineError::EffectValue(
                    "'$exec.response.*' kaldırıldı (M7) — '$exec.result.*' kullanın".into(),
                ));
            }
            Ok(Value::from(s))
        }
    }
}

/// Dotted path okuma.
pub fn get_path<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = value;
    for part in dotted.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

/// Dotted path yazma — ara segmentler obje değilse objeyle değiştirilir.
pub fn set_path(root: &mut Map<String, Value>, dotted: &str, value: Value) {
    let mut parts = dotted.split('.').peekable();
    let mut current = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_string(), value);
            return;
        }
        let entry = current
            .entry(part.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("az önce obje yapıldı");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::wfd_v22::WfesEffects;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::nil(),
            user_id: Uuid::nil(),
            role: "creditAnalyst".into(),
        }
    }

    fn env<'a>(a: &'a Actor, input: Option<&'a Value>, exec: Option<&'a Value>) -> EffectEnv<'a> {
        EffectEnv {
            actor: a,
            wfe_id: Uuid::nil(),
            node: Some("self__creditAnalyst"),
            action_input: input,
            exec_result: exec,
            now: Utc::now(),
        }
    }

    fn effects(pairs: &[(&str, Value)]) -> WfesEffects {
        WfesEffects {
            set: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn special_dollar_strings_resolve() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({}),
            &effects(&[
                ("initiated_by", json!("$actor")),
                ("at", json!("$timestamp")),
                ("wfe", json!("$wfe_id")),
                ("node", json!("$node")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["initiated_by"]["role"], json!("creditAnalyst"));
        assert_eq!(out["wfe"], json!(Uuid::nil().to_string()));
        assert_eq!(out["node"], json!("self__creditAnalyst"));
        assert!(out["at"].as_str().unwrap().contains('T'));
    }

    #[test]
    fn ctx_action_exec_refs_resolve() {
        let a = actor();
        let input = json!({"manager_decision": "approve"});
        let exec = json!({"score": 740, "grade": "A"});
        let e = env(&a, Some(&input), Some(&exec));
        let ctx = json!({"credit_info": {"amount_requested": 5000}});
        let out = apply_effects(
            &ctx,
            &effects(&[
                ("amount", json!("$ctx.credit_info.amount_requested")),
                ("decision", json!("$action.input.manager_decision")),
                ("credit_score", json!("$exec.result.score")),
                ("credit_grade", json!("$exec.result.grade")),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["amount"], json!(5000));
        assert_eq!(out["decision"], json!("approve"));
        assert_eq!(out["credit_score"], json!(740));
        assert_eq!(out["credit_grade"], json!("A"));
    }

    #[test]
    fn missing_refs_become_null() {
        let a = actor();
        let e = env(&a, None, None);
        let out =
            apply_effects(&json!({}), &effects(&[("x", json!("$ctx.ghost.path"))]), &e).unwrap();
        assert_eq!(out["x"], Value::Null);
    }

    #[test]
    fn exec_response_namespace_is_error() {
        let a = actor();
        let e = env(&a, None, None);
        let err = apply_effects(
            &json!({}),
            &effects(&[("x", json!("$exec.response.score"))]),
            &e,
        )
        .unwrap_err();
        assert!(err.to_string().contains("$exec.result"));
    }

    #[test]
    fn dotted_set_path_creates_nested_objects() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({"credit_info": {"purpose": "ev"}}),
            &effects(&[("credit_info.amount_requested", json!(9000))]),
            &e,
        )
        .unwrap();
        assert_eq!(out["credit_info"]["amount_requested"], json!(9000));
        assert_eq!(
            out["credit_info"]["purpose"],
            json!("ev"),
            "kardeş alan korunmalı"
        );
    }

    #[test]
    fn plain_strings_and_literals_pass_through() {
        let a = actor();
        let e = env(&a, None, None);
        let out = apply_effects(
            &json!({}),
            &effects(&[
                ("note", json!("düz metin")),
                ("n", json!(42)),
                ("b", json!(true)),
            ]),
            &e,
        )
        .unwrap();
        assert_eq!(out["note"], json!("düz metin"));
        assert_eq!(out["n"], json!(42));
        assert_eq!(out["b"], json!(true));
    }

    #[test]
    fn original_ctx_is_not_mutated() {
        let a = actor();
        let e = env(&a, None, None);
        let ctx = json!({"a": 1});
        let _ = apply_effects(&ctx, &effects(&[("a", json!(2))]), &e).unwrap();
        assert_eq!(ctx["a"], json!(1));
    }
}
