use thiserror::Error;

#[derive(Debug, Error)]
pub enum AutoexecError {
    #[error("REST request failed: {0}")]
    RestRequestFailed(String),

    #[error("SQL query execution failed: {0}")]
    SqlExecutionFailed(String),

    #[error("Database connection failed: {0}")]
    DatabaseConnectionFailed(String),

    #[error("Schema validation failed: {0}")]
    SchemaValidationFailed(String),

    #[error("Parameter mapping failed: {0}")]
    ParameterMappingFailed(String),

    #[error("Result mapping failed: {0}")]
    ResultMappingFailed(String),

    #[error("Expression evaluation failed: {0}")]
    ExpressionEvaluationFailed(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("JSON parse error: {0}")]
    JsonParseError(#[from] serde_json::Error),
}
