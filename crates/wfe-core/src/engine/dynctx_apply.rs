use crate::{
    error::EngineError,
    types::{
        actor::Actor,
        dynctx::DynCtx,
        wfd::{EffectValue, WfesEffects},
    },
};
use chrono::Utc;
use serde_json::{json, Value};
use uuid::Uuid;

/// Applies wfes_effects to produce a new immutable DynCtx. Never mutates `ctx`.
pub fn apply(
    ctx: &DynCtx,
    effects: &WfesEffects,
    actor: &Actor,
    wfe_id: Uuid,
    action: &str,
    input: &Value,
) -> Result<DynCtx, EngineError> {
    let mut patch = serde_json::Map::new();
    for (field, effect_val) in &effects.set {
        let resolved = resolve(effect_val, actor, wfe_id, action, input, ctx)?;
        patch.insert(field.clone(), resolved);
    }
    for (field, effect_val) in &effects.append {
        let resolved = resolve(effect_val, actor, wfe_id, action, input, ctx)?;
        let next = match ctx.get(field) {
            Some(Value::Array(existing)) => {
                let mut values = existing.clone();
                values.push(resolved);
                Value::Array(values)
            }
            Some(existing) => Value::Array(vec![existing.clone(), resolved]),
            None => Value::Array(vec![resolved]),
        };
        patch.insert(field.clone(), next);
    }

    // Always expose the executing actor under _step_<action> so downstream
    // c_orgu rules can reference it via from: "$ctx._step_<action>.actor.orgu"
    // without requiring explicit wfes_effects.set["key"] = "$actor".
    let step_key = format!("_step_{}", action);
    patch.insert(
        step_key,
        json!({
            "actor": {
                "orgu":    actor.orgu_id,
                "user":    actor.user_id,
                "orgu_id": actor.orgu_id,
                "user_id": actor.user_id,
                "role":    actor.role,
            }
        }),
    );

    Ok(ctx.merge(patch))
}

fn resolve(
    val: &EffectValue,
    actor: &Actor,
    wfe_id: Uuid,
    action: &str,
    input: &Value,
    ctx: &DynCtx,
) -> Result<Value, EngineError> {
    match val {
        EffectValue::Ref { path } => resolve_ctx_ref(path, ctx),
        EffectValue::Ctx { ctx: path } => resolve_ctx_ref(path, ctx),
        EffectValue::Literal(v) => {
            if let Some(s) = v.as_str() {
                Ok(resolve_special(s, actor, wfe_id, action, input))
            } else {
                Ok(v.clone())
            }
        }
    }
}

fn resolve_special(s: &str, actor: &Actor, wfe_id: Uuid, _action: &str, input: &Value) -> Value {
    match s {
        "$actor" => json!({
            "orgu":    actor.orgu_id,
            "user":    actor.user_id,
            "orgu_id": actor.orgu_id,
            "user_id": actor.user_id,
            "role":    actor.role,
        }),
        "$timestamp" => json!(Utc::now().to_rfc3339()),
        "$wfe_id" => json!(wfe_id),
        s if s.starts_with("$action.input.") => {
            let field = &s["$action.input.".len()..];
            input.get(field).cloned().unwrap_or(Value::Null)
        }
        _ => Value::String(s.to_string()),
    }
}

fn resolve_ctx_ref(path: &str, ctx: &DynCtx) -> Result<Value, EngineError> {
    let stripped = path.strip_prefix("$ctx.").unwrap_or(path);

    let mut current = ctx.as_value();
    for part in stripped.split('.') {
        current = current
            .get(part)
            .ok_or_else(|| EngineError::EffectValue(format!("ctx ref path not found: {path}")))?;
    }
    Ok(current.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        actor::Actor,
        dynctx::DynCtx,
        wfd::{EffectValue, WfesEffects},
    };
    use serde_json::json;
    use uuid::Uuid;

    fn actor() -> Actor {
        Actor {
            orgu_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "clerk".into(),
        }
    }

    #[test]
    fn sets_literal_string() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects
            .set
            .insert("status".into(), EffectValue::Literal(json!("pending")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert_eq!(new_ctx.get("status"), Some(&json!("pending")));
    }

    #[test]
    fn sets_actor_special() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects
            .set
            .insert("initiated_by".into(), EffectValue::Literal(json!("$actor")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        let stored = new_ctx.get("initiated_by").unwrap();
        assert_eq!(stored["orgu_id"], json!(actor.orgu_id));
        assert_eq!(stored["role"], json!("clerk"));
    }

    #[test]
    fn sets_action_input_ref() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert(
            "amount".into(),
            EffectValue::Literal(json!("$action.input.amount")),
        );
        let input = json!({"amount": 500});

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "submit", &input).unwrap();
        assert_eq!(new_ctx.get("amount"), Some(&json!(500)));
    }

    #[test]
    fn auto_injects_step_actor() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let effects = WfesEffects::default();

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "approve", &json!({})).unwrap();
        let step = new_ctx.get("_step_approve").unwrap();
        assert_eq!(step["actor"]["orgu_id"], json!(actor.orgu_id));
        assert_eq!(step["actor"]["role"], json!("clerk"));
    }

    #[test]
    fn original_ctx_unchanged() {
        let ctx = DynCtx::empty();
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects
            .set
            .insert("status".into(), EffectValue::Literal(json!("pending")));

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert!(ctx.get("status").is_none());
        assert!(new_ctx.get("status").is_some());
    }

    #[test]
    fn ctx_ref_reads_existing_ctx_field() {
        let ctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("applicant_orgu".into(), json!({"orgu": "abc-uuid"}));
            m
        });
        let wfe_id = Uuid::new_v4();
        let actor = actor();
        let mut effects = WfesEffects::default();
        effects.set.insert(
            "orgu_copy".into(),
            EffectValue::Ref {
                path: "$ctx.applicant_orgu.orgu".into(),
            },
        );

        let new_ctx = apply(&ctx, &effects, &actor, wfe_id, "start", &json!({})).unwrap();
        assert_eq!(new_ctx.get("orgu_copy"), Some(&json!("abc-uuid")));
    }
}
