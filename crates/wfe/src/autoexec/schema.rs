use serde_json::{Value, Map};
use std::collections::HashMap;
use crate::autoexec::error::AutoexecError;

#[derive(Debug, Clone)]
pub struct InputMapping {
    pub mappings: HashMap<String, MappingExpr>,
}

#[derive(Debug, Clone)]
pub enum MappingExpr {
    Ref(String),
    Literal(Value),
    Ctx(String),
}

#[derive(Debug, Clone)]
pub struct OutputMapping {
    pub mappings: HashMap<String, String>,
}

pub struct SchemaValidator;

impl SchemaValidator {
    pub fn validate_input(
        input_mapping: &InputMapping,
        dynctx: &Value,
    ) -> Result<Value, AutoexecError> {
        let mut params = Map::new();

        for (key, expr) in &input_mapping.mappings {
            let value = match expr {
                MappingExpr::Ref(path) => {
                    Self::resolve_ref(path, dynctx)?
                }
                MappingExpr::Literal(v) => v.clone(),
                MappingExpr::Ctx(path) => {
                    Self::resolve_ctx_path(path, dynctx)?
                }
            };
            params.insert(key.clone(), value);
        }

        Ok(Value::Object(params))
    }

    pub fn validate_output(
        response: &Value,
        output_mapping: &OutputMapping,
    ) -> Result<Value, AutoexecError> {
        let mut result = Map::new();

        for (key, jsonpath) in &output_mapping.mappings {
            let value = Self::extract_jsonpath(response, jsonpath)?;
            result.insert(key.clone(), value);
        }

        Ok(Value::Object(result))
    }

    fn resolve_ref(path: &str, dynctx: &Value) -> Result<Value, AutoexecError> {
        if path.starts_with("$ctx.") {
            let field_path = &path[5..];
            Self::resolve_ctx_path(field_path, dynctx)
        } else {
            Err(AutoexecError::ParameterMappingFailed(
                format!("Invalid ref format: {}", path),
            ))
        }
    }

    fn resolve_ctx_path(path: &str, dynctx: &Value) -> Result<Value, AutoexecError> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = dynctx.clone();

        for part in parts {
            match current.get(part) {
                Some(v) => current = v.clone(),
                None => {
                    return Err(AutoexecError::ParameterMappingFailed(
                        format!("Path not found in context: {}", path),
                    ))
                }
            }
        }

        Ok(current)
    }

    pub fn extract_jsonpath(response: &Value, path: &str) -> Result<Value, AutoexecError> {
        if path == "$" {
            return Ok(response.clone());
        }

        if path.starts_with("$.") {
            let path_expr = &path[2..];
            Self::extract_nested_path(response, path_expr)
        } else {
            Err(AutoexecError::ResultMappingFailed(
                format!("Invalid JSONPath: {}", path),
            ))
        }
    }

    fn extract_nested_path(value: &Value, path: &str) -> Result<Value, AutoexecError> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = value.clone();

        for part in parts {
            if let Some(idx_end) = part.find('[') {
                let key = &part[..idx_end];
                let idx_str = &part[idx_end + 1..];
                let idx_str = idx_str.trim_end_matches(']');

                if let Some(obj) = current.get_mut(key) {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        current = obj[idx].clone();
                    } else {
                        return Err(AutoexecError::ResultMappingFailed(
                            format!("Invalid array index: {}", idx_str),
                        ));
                    }
                } else {
                    return Err(AutoexecError::ResultMappingFailed(
                        format!("Path not found: {}", key),
                    ));
                }
            } else if let Some(v) = current.get(part) {
                current = v.clone();
            } else {
                return Err(AutoexecError::ResultMappingFailed(
                    format!("Path not found: {}", part),
                ));
            }
        }

        Ok(current)
    }

    pub fn parse_input_mapping(params: &Value) -> Result<InputMapping, AutoexecError> {
        let mut mappings = HashMap::new();

        if let Some(obj) = params.as_object() {
            for (key, value) in obj {
                let expr = if let Some(ref_obj) = value.as_object() {
                    if let Some(ref_str) = ref_obj.get("ref") {
                        if let Some(s) = ref_str.as_str() {
                            MappingExpr::Ref(s.to_string())
                        } else {
                            return Err(AutoexecError::InvalidConfiguration(
                                "Invalid ref format".to_string(),
                            ));
                        }
                    } else if let Some(ctx_str) = ref_obj.get("ctx") {
                        if let Some(s) = ctx_str.as_str() {
                            MappingExpr::Ctx(s.to_string())
                        } else {
                            return Err(AutoexecError::InvalidConfiguration(
                                "Invalid ctx format".to_string(),
                            ));
                        }
                    } else {
                        MappingExpr::Literal(value.clone())
                    }
                } else {
                    MappingExpr::Literal(value.clone())
                };
                mappings.insert(key.clone(), expr);
            }
        }

        Ok(InputMapping { mappings })
    }

    pub fn parse_output_mapping(result: &Value) -> Result<OutputMapping, AutoexecError> {
        let mut mappings = HashMap::new();

        if let Some(obj) = result.as_object() {
            for (key, value) in obj {
                if let Some(jsonpath) = value.as_str() {
                    mappings.insert(key.clone(), jsonpath.to_string());
                } else {
                    return Err(AutoexecError::InvalidConfiguration(
                        format!("Invalid output mapping for key: {}", key),
                    ));
                }
            }
        }

        Ok(OutputMapping { mappings })
    }
}
