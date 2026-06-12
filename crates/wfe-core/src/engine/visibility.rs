use crate::types::{actor::Actor, dynctx::DynCtx, wfd::WFD};
use serde_json::Value;

/// V(DynCtx, Actor) → filtered DynCtx value
/// Applies x-visibility rules from the WFD context schema.
/// Fields without x-visibility are visible by default.
pub fn apply(dynctx: &DynCtx, actor: &Actor, wfd: &WFD) -> Value {
    let schema = &wfd.context;
    let props = match schema.get("properties") {
        Some(Value::Object(p)) => p,
        _ => return dynctx.as_value().clone(),
    };

    let mut result = serde_json::Map::new();
    if let Value::Object(ctx_map) = dynctx.as_value() {
        for (field, value) in ctx_map {
            let visible = match props.get(field).and_then(|s| s.get("x-visibility")) {
                None => true,
                Some(rule) => actor_matches_visibility(rule, actor),
            };
            if visible {
                result.insert(field.clone(), value.clone());
            }
        }
    }
    Value::Object(result)
}

fn actor_matches_visibility(rule: &Value, actor: &Actor) -> bool {
    if let Some(c_r) = rule.get("c_r").and_then(|v| v.as_array()) {
        for item in c_r {
            let role = item
                .as_str()
                .or_else(|| {
                    item.as_array()
                        .and_then(|arr| arr.get(1))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("");
            if role == actor.role {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn actor(role: &str) -> Actor {
        Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: role.into(),
        }
    }

    fn minimal_wfd(context: Value) -> WFD {
        WFD {
            id: Uuid::new_v4().to_string(),
            name: "t".into(),
            version: "1.0.0".into(),
            description: None,
            context,
            start: vec![],
            actions: Default::default(),
            transitions: vec![],
            listable: vec![],
            terminal_when: "false".into(),
            extra: Default::default(),
        }
    }

    #[test]
    fn no_visibility_rule_always_visible() {
        let wfd = minimal_wfd(json!({"properties": {"status": {"type": "string"}}}));
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), json!("pending"));
            m
        });
        let result = apply(&ctx, &actor("clerk"), &wfd);
        assert_eq!(result["status"], json!("pending"));
    }

    #[test]
    fn visibility_rule_hides_field_for_wrong_role() {
        let wfd = minimal_wfd(json!({
            "properties": {
                "secret": {
                    "type": "string",
                    "x-visibility": {"c_r": [["self", "manager"]]}
                }
            }
        }));
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("secret".into(), json!("hidden-value"));
            m
        });

        let result = apply(&ctx, &actor("clerk"), &wfd);
        assert!(result.get("secret").is_none());

        let result2 = apply(&ctx, &actor("manager"), &wfd);
        assert_eq!(result2["secret"], json!("hidden-value"));
    }
}
