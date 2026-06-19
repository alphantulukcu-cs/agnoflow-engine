#[cfg(test)]
mod tests {
    use serde_json::json;

    mod schema_validator {
        use super::super::super::schema::*;
        use serde_json::json;

        #[test]
        fn test_extract_jsonpath() {
            let response = json!({
                "data": {
                    "user": {
                        "id": 123,
                        "name": "Alice"
                    }
                }
            });

            let result = SchemaValidator::extract_jsonpath(&response, "$.data.user.id").unwrap();
            assert_eq!(result, json!(123));

            let result = SchemaValidator::extract_jsonpath(&response, "$.data.user.name").unwrap();
            assert_eq!(result, json!("Alice"));
        }

        #[test]
        fn test_parse_input_mapping() {
            let params = json!({
                "url_param": { "ref": "$ctx.user.id" },
                "literal_value": 42
            });

            let mapping = SchemaValidator::parse_input_mapping(&params).unwrap();
            assert_eq!(mapping.mappings.len(), 2);
            assert!(mapping.mappings.contains_key("url_param"));
            assert!(mapping.mappings.contains_key("literal_value"));
        }

        #[test]
        fn test_parse_output_mapping() {
            let result = json!({
                "user_id": "$.data.user.id",
                "user_name": "$.data.user.name"
            });

            let mapping = SchemaValidator::parse_output_mapping(&result).unwrap();
            assert_eq!(mapping.mappings.len(), 2);
            assert_eq!(mapping.mappings.get("user_id"), Some(&"$.data.user.id".to_string()));
        }
    }

    mod calc_executor {
        use serde_json::json;

        #[test]
        fn test_calc_executor_creation() {
            // Test that CalcExecutor can be instantiated
            let _executor = super::super::super::calc::CalcExecutor::new();
        }
    }

    mod autoexec_executor {
        use super::super::super::{AutoexecExecutor, RestExecutor, SqlExecutor, CalcExecutor};
        use serde_json::json;

        #[test]
        fn test_autoexec_executor_creation() {
            let _executor = AutoexecExecutor::new(
                RestExecutor::new(),
                SqlExecutor::new(None),
                CalcExecutor::new(),
            );
            // Executor created successfully
        }
    }
}
