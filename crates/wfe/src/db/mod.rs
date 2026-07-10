pub mod crypto;
pub mod drivers;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbDriver { Postgres, Mysql, Mssql }

impl DbDriver {
    pub fn parse(s: &str) -> Option<Self> {
        match s { "postgres" => Some(Self::Postgres), "mysql" => Some(Self::Mysql), "mssql" => Some(Self::Mssql), _ => None }
    }
}

/// Test/çalıştırma için çözülmüş (secret düz metin) bağlantı bilgisi.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub driver:   DbDriver,
    pub mode:     String,           // "fields" | "uri"
    pub host:     Option<String>,
    pub port:     Option<i32>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub secret:   Option<String>,   // parola (fields) veya bağlantı dizesi (uri)
    pub options:  serde_json::Value,
}

#[derive(Debug)]
pub struct DbError(pub String);
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.0) }
}
impl std::error::Error for DbError {}
