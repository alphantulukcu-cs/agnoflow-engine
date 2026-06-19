use serde_json::{Value, Map, json};
use wfe_core::types::wfd::AutoexecDef;

use crate::autoexec::{AutoexecContext, AutoexecResult};
use crate::autoexec::error::AutoexecError;
use crate::error::WfeError;

pub struct CalcExecutor;

impl CalcExecutor {
    pub fn new() -> Self {
        Self
    }

    pub async fn execute(
        &self,
        def: &AutoexecDef,
        ctx: &AutoexecContext,
    ) -> Result<AutoexecResult, WfeError> {
        let config = self.parse_config(&def.params)?;

        let mut result = Map::new();

        for (key, expr) in config.expressions {
            let value = self.evaluate_expression(&expr, &ctx.dynctx)?;
            result.insert(key, value);
        }

        Ok(AutoexecResult {
            result: Value::Object(result),
        })
    }

    fn parse_config(&self, params: &Value) -> Result<CalcConfig, WfeError> {
        let expressions = params
            .get("expressions")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                WfeError::AutoexecError(AutoexecError::InvalidConfiguration(
                    "Missing 'expressions' in CALC config".to_string(),
                ))
            })?
            .iter()
            .map(|(k, v)| {
                let expr = v
                    .as_str()
                    .ok_or_else(|| {
                        WfeError::AutoexecError(AutoexecError::InvalidConfiguration(
                            format!("Expression for '{}' must be a string", k),
                        ))
                    })?
                    .to_string();
                Ok((k.clone(), expr))
            })
            .collect::<Result<std::collections::HashMap<_, _>, WfeError>>()?;

        Ok(CalcConfig { expressions })
    }

    fn evaluate_expression(&self, expr: &str, ctx: &Value) -> Result<Value, WfeError> {
        let zen_expr = self.convert_to_zen_expr(expr)?;

        // Build context with $ prefix for zen-expression
        let mut context_map = serde_json::Map::new();
        if let Some(obj) = ctx.as_object() {
            for (k, v) in obj {
                context_map.insert(format!("${}", k), v.clone());
            }
        }
        let context = Value::Object(context_map);

        match zen_expression::evaluate_expression(&zen_expr, context.into()) {
            Ok(variable) => {
                // Convert zen_expression::Variable to serde_json::Value
                let value = match variable {
                    zen_expression::Variable::Number(n) => json!(n),
                    zen_expression::Variable::String(s) => json!(s),
                    zen_expression::Variable::Bool(b) => json!(b),
                    _ => json!(null),
                };
                Ok(value)
            },
            Err(e) => Err(WfeError::AutoexecError(
                AutoexecError::ExpressionEvaluationFailed(e.to_string()),
            )),
        }
    }

    fn convert_to_zen_expr(&self, expr: &str) -> Result<String, WfeError> {
        // Convert from calculator syntax to ZEN syntax
        // ctx.field -> $field
        // ctx.field.nested -> $field.nested
        let zen_expr = expr
            .replace("ctx.", "$")
            .replace(" and ", " && ")
            .replace(" or ", " || ");

        Ok(zen_expr)
    }
}

impl Default for CalcExecutor {
    fn default() -> Self {
        Self::new()
    }
}

struct CalcConfig {
    expressions: std::collections::HashMap<String, String>,
}
