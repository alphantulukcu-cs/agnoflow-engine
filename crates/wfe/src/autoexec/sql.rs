use serde_json::{json, Value};
use sqlx::{Postgres, Pool};
use wfe_core::types::wfd::AutoexecDef;

use crate::autoexec::{AutoexecContext, AutoexecResult};
use crate::autoexec::error::AutoexecError;
use crate::autoexec::schema::SchemaValidator;
use crate::error::WfeError;

pub struct SqlExecutor {
    postgres_pool: Option<Pool<Postgres>>,
}

impl SqlExecutor {
    pub fn new(postgres_pool: Option<Pool<Postgres>>) -> Self {
        Self {
            postgres_pool,
        }
    }

    pub async fn execute(
        &self,
        def: &AutoexecDef,
        ctx: &AutoexecContext,
    ) -> Result<AutoexecResult, WfeError> {
        let config = self.parse_config(&def.params)?;

        // Parse input/output mappings
        let input_mapping = SchemaValidator::parse_input_mapping(&config.params)?;
        let output_mapping = SchemaValidator::parse_output_mapping(&config.result)?;

        // Validate and map input parameters
        let params = SchemaValidator::validate_input(&input_mapping, &ctx.dynctx)?;

        // Substitute query parameters
        let query = self.substitute_query(&config.query, &params)?;

        // Execute query
        let response = self.execute_query(&query, &config.database_type).await?;

        // Map output
        let result = SchemaValidator::validate_output(&response, &output_mapping)?;

        Ok(AutoexecResult { result })
    }

    fn parse_config(&self, params: &Value) -> Result<SqlConfig, WfeError> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                WfeError::AutoexecError(AutoexecError::InvalidConfiguration(
                    "Missing 'query' in SQL config".to_string(),
                ))
            })?
            .to_string();

        let params_value = params.get("params").cloned().unwrap_or(json!({}));
        let result_value = params.get("result").cloned().unwrap_or(json!({}));

        let database_type = params
            .get("database_type")
            .and_then(|v| v.as_str())
            .unwrap_or("postgres")
            .to_string();

        Ok(SqlConfig {
            query,
            params: params_value,
            result: result_value,
            database_type,
        })
    }

    fn substitute_query(&self, query: &str, params: &Value) -> Result<String, WfeError> {
        let mut result = query.to_string();

        if let Some(obj) = params.as_object() {
            for (key, value) in obj {
                let placeholder = format!(":{}", key);
                let value_str = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "NULL".to_string(),
                    other => other.to_string(),
                };

                result = result.replace(&placeholder, &format!("'{}'", value_str));
            }
        }

        Ok(result)
    }

    async fn execute_query(
        &self,
        query: &str,
        database_type: &str,
    ) -> Result<Value, WfeError> {
        match database_type {
            "postgres" => self.execute_postgres(query).await,
            "mysql" => Err(WfeError::AutoexecError(AutoexecError::SqlExecutionFailed(
                "MySQL support coming soon".to_string(),
            ))),
            "sqlite" => Err(WfeError::AutoexecError(AutoexecError::SqlExecutionFailed(
                "SQLite support coming soon".to_string(),
            ))),
            _ => Err(WfeError::AutoexecError(AutoexecError::InvalidConfiguration(
                format!("Unsupported database type: {}", database_type),
            ))),
        }
    }

    async fn execute_postgres(&self, query: &str) -> Result<Value, WfeError> {
        let pool = self.postgres_pool.as_ref().ok_or_else(|| {
            WfeError::AutoexecError(AutoexecError::DatabaseConnectionFailed(
                "PostgreSQL pool not configured".to_string(),
            ))
        })?;

        // Execute generic query - convert results to JSON
        use sqlx::Row as RowTrait;

        let rows = sqlx::query(query)
            .fetch_all(pool)
            .await
            .map_err(|e| {
                WfeError::AutoexecError(AutoexecError::SqlExecutionFailed(e.to_string()))
            })?;

        if rows.is_empty() {
            return Ok(json!(null));
        }

        // Try to extract all values as JSON
        let mut results = Vec::new();
        for row in rows {
            let mut obj = serde_json::Map::new();
            for (idx, _column) in row.columns().iter().enumerate() {
                let col_name = format!("{}", idx); // Use ordinal as fallback
                let value: Value = match row.try_get::<String, usize>(idx) {
                    Ok(v) => json!(v),
                    Err(_) => {
                        match row.try_get::<i64, usize>(idx) {
                            Ok(v) => json!(v),
                            Err(_) => {
                                match row.try_get::<f64, usize>(idx) {
                                    Ok(v) => json!(v),
                                    Err(_) => {
                                        match row.try_get::<bool, usize>(idx) {
                                            Ok(v) => json!(v),
                                            Err(_) => Value::Null,
                                        }
                                    }
                                }
                            }
                        }
                    }
                };
                obj.insert(col_name, value);
            }
            results.push(Value::Object(obj));
        }

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::Array(results))
        }
    }
}

impl Default for SqlExecutor {
    fn default() -> Self {
        Self::new(None)
    }
}

struct SqlConfig {
    query: String,
    params: Value,
    result: Value,
    database_type: String,
}
