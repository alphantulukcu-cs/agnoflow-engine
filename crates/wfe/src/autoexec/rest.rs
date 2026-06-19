use reqwest::{Client, Method};
use serde_json::{json, Value};
use std::str::FromStr;
use std::time::Duration;
use wfe_core::types::wfd::AutoexecDef;

use crate::autoexec::{AutoexecContext, AutoexecResult};
use crate::autoexec::error::AutoexecError;
use crate::autoexec::schema::SchemaValidator;
use crate::error::WfeError;

pub struct RestExecutor {
    client: Client,
}

impl RestExecutor {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self { client }
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

        // Build and execute request
        let url = self.substitute_url(&config.url, &params)?;
        let method = Method::from_str(&config.method)
            .map_err(|_| WfeError::AutoexecError(
                AutoexecError::InvalidConfiguration(format!("Invalid HTTP method: {}", config.method))
            ))?;

        let response = self.execute_request(&url, method, &config, &params).await?;

        // Map output
        let result = SchemaValidator::validate_output(&response, &output_mapping)?;

        Ok(AutoexecResult { result })
    }

    fn parse_config(&self, params: &Value) -> Result<RestConfig, WfeError> {
        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET")
            .to_string();

        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                WfeError::AutoexecError(AutoexecError::InvalidConfiguration(
                    "Missing 'url' in REST config".to_string(),
                ))
            })?
            .to_string();

        let params_value = params.get("params").cloned().unwrap_or(json!({}));
        let result_value = params.get("result").cloned().unwrap_or(json!({}));

        Ok(RestConfig {
            method,
            url,
            params: params_value,
            result: result_value,
        })
    }

    fn substitute_url(&self, url: &str, params: &Value) -> Result<String, WfeError> {
        let mut result = url.to_string();

        if let Some(obj) = params.as_object() {
            for (key, value) in obj {
                let placeholder = format!("{{{{{}}}}}", key);
                let value_str = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                };
                result = result.replace(&placeholder, &value_str);
            }
        }

        Ok(result)
    }

    async fn execute_request(
        &self,
        url: &str,
        method: Method,
        _config: &RestConfig,
        params: &Value,
    ) -> Result<Value, WfeError> {
        let request = match method {
            Method::GET => {
                let mut req = self.client.get(url);
                if let Some(obj) = params.as_object() {
                    for (key, value) in obj {
                        req = req.query(&[(key, value.to_string())]);
                    }
                }
                req.build()
            }
            Method::POST | Method::PUT => {
                let req = if method == Method::POST {
                    self.client.post(url)
                } else {
                    self.client.put(url)
                };
                req.json(params).build()
            }
            Method::DELETE => self.client.delete(url).build(),
            _ => return Err(WfeError::AutoexecError(
                AutoexecError::InvalidConfiguration("Unsupported HTTP method".to_string())
            )),
        };

        let request = request.map_err(|e| {
            WfeError::AutoexecError(AutoexecError::RestRequestFailed(e.to_string()))
        })?;

        let response = self.client
            .execute(request)
            .await
            .map_err(|e| {
                WfeError::AutoexecError(AutoexecError::RestRequestFailed(e.to_string()))
            })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| {
                WfeError::AutoexecError(AutoexecError::RestRequestFailed(e.to_string()))
            })?;

        if !status.is_success() {
            return Err(WfeError::AutoexecError(AutoexecError::RestRequestFailed(
                format!("HTTP {}: {}", status, body),
            )));
        }

        serde_json::from_str(&body).map_err(|e| {
            WfeError::AutoexecError(AutoexecError::RestRequestFailed(
                format!("Failed to parse response: {}", e),
            ))
        })
    }
}

impl Default for RestExecutor {
    fn default() -> Self {
        Self::new()
    }
}

struct RestConfig {
    method: String,
    url: String,
    params: Value,
    result: Value,
}
