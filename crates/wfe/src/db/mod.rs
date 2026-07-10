pub mod crypto;
pub mod drivers;
pub mod run;

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

/// `:name` yer tutucularını sıralı (marker, value) listesine çevirir.
/// marker_fn(index) sürücüye göre işaret üretir ($1 / ? / @P1).
pub fn bind_params(
    query: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    marker: impl Fn(usize) -> String,
) -> (String, Vec<serde_json::Value>) {
    let mut sql = query.to_string();
    let mut bound = Vec::new();
    for (name, value) in params {
        let ph = format!(":{name}");
        if sql.contains(&ph) {
            bound.push(value.clone());
            sql = sql.replace(&ph, &marker(bound.len()));
        }
    }
    (sql, bound)
}

#[cfg(test)]
mod bind_tests {
    use super::bind_params;
    use serde_json::json;

    #[test]
    fn pg_markers_positional() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!(1));
        let (sql, vals) = bind_params("SELECT * WHERE x = :a", &m, |i| format!("${i}"));
        assert_eq!(sql, "SELECT * WHERE x = $1");
        assert_eq!(vals, vec![json!(1)]);
    }

    #[test]
    fn mysql_marker_qmark() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!("x"));
        let (sql, _) = bind_params("WHERE x = :a", &m, |_| "?".to_string());
        assert_eq!(sql, "WHERE x = ?");
    }
}
