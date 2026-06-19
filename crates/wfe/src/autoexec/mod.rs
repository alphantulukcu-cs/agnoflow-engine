pub mod rest;
pub mod sql;
pub mod calc;
pub mod schema;
pub mod error;
#[cfg(test)]
mod tests;

pub use rest::RestExecutor;
pub use sql::SqlExecutor;
pub use calc::CalcExecutor;
pub use schema::{SchemaValidator, InputMapping, OutputMapping};

use serde_json::Value;
use uuid::Uuid;
use wfe_core::types::wfd::AutoexecDef;
use crate::error::WfeError;

#[derive(Debug, Clone)]
pub struct AutoexecContext {
    pub wfe_id: Uuid,
    pub dynctx: Value,
    pub params: Value,
}

#[derive(Debug)]
pub struct AutoexecResult {
    pub result: Value,
}

#[async_trait::async_trait]
pub trait AutoexecHandler: Send + Sync {
    async fn execute(&self, def: &AutoexecDef, ctx: &AutoexecContext) -> Result<AutoexecResult, WfeError>;
}

pub struct AutoexecExecutor {
    rest: RestExecutor,
    sql: SqlExecutor,
    calc: CalcExecutor,
}

impl AutoexecExecutor {
    pub fn new(rest: RestExecutor, sql: SqlExecutor, calc: CalcExecutor) -> Self {
        Self { rest, sql, calc }
    }

    pub async fn execute(
        &self,
        def: &AutoexecDef,
        ctx: &AutoexecContext,
    ) -> Result<Value, WfeError> {
        let result = match def.kind.as_str() {
            "rest" => self.rest.execute(def, ctx).await?,
            "sql" => self.sql.execute(def, ctx).await?,
            "calc" => self.calc.execute(def, ctx).await?,
            _ => return Err(WfeError::UnknownAutoexecType(def.kind.clone())),
        };

        Ok(result.result)
    }
}
