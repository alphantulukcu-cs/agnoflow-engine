use crate::{error::EngineError, ports::WFES};

/// Evaluates a ZEN expression against the current WFES.
/// DynCtx fields are exposed as `$<field>`, WFAH as `$wfah`.
pub fn evaluate(expr: &str, wfes: &WFES) -> Result<bool, EngineError> {
    let context = build_context(wfes);
    eval_expression(expr, context)
}

fn build_context(wfes: &WFES) -> serde_json::Value {
    let mut map = serde_json::Map::new();

    if let serde_json::Value::Object(ctx_map) = wfes.dynctx.as_value() {
        for (k, v) in ctx_map {
            map.insert(format!("${k}"), v.clone());
        }
    }

    let wfah_arr: serde_json::Value = wfes.wfah.entries()
        .iter()
        .map(|e| serde_json::json!({
            "action":     e.action,
            "actor":      e.actor,
            "applied_at": e.applied_at.to_rfc3339(),
        }))
        .collect();
    map.insert("$wfah".into(), wfah_arr);

    serde_json::Value::Object(map)
}

fn eval_expression(expr: &str, context: serde_json::Value) -> Result<bool, EngineError> {
    let result = zen_expression::evaluate_expression(expr, context.into())
        .map_err(|e| EngineError::ZenEvaluation(e.to_string()))?;

    result
        .as_bool()
        .ok_or_else(|| EngineError::ZenEvaluation(
            format!("expression '{expr}' did not evaluate to boolean")
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use crate::{ports::WFES, types::{dynctx::DynCtx, wfah::Wfah, wfe::WfeStatus}};

    fn wfes(status: &str) -> WFES {
        let dynctx = DynCtx::empty().merge({
            let mut m = serde_json::Map::new();
            m.insert("status".into(), serde_json::json!(status));
            m
        });
        WFES {
            wfe_id: Uuid::new_v4(), dynctx, wfah: Wfah::empty(),
            status: WfeStatus::Active, orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(), wfd_version: 1,
            current_c_a: vec![], end_response: None,
        }
    }

    #[test]
    fn evaluates_true_condition() {
        let w = wfes("pending");
        assert!(evaluate("$status == 'pending'", &w).unwrap());
    }

    #[test]
    fn evaluates_false_condition() {
        let w = wfes("pending");
        assert!(!evaluate("$status == 'approved'", &w).unwrap());
    }

    #[test]
    fn numeric_comparison() {
        let mut m = serde_json::Map::new();
        m.insert("amount".into(), serde_json::json!(500));
        let dynctx = DynCtx::empty().merge(m);
        let w = WFES {
            wfe_id: Uuid::new_v4(), dynctx, wfah: Wfah::empty(),
            status: WfeStatus::Active, orgtnt_id: Uuid::new_v4(),
            wfd_id: Uuid::new_v4(), wfd_version: 1,
            current_c_a: vec![], end_response: None,
        };
        assert!(evaluate("$amount < 1000", &w).unwrap());
        assert!(!evaluate("$amount >= 1000", &w).unwrap());
    }
}
