pub mod crypto;
pub mod drivers;
pub mod run;

use serde::{Deserialize, Serialize};

/// Wire protokolü. UI'daki driver adları alias olarak buna çözülür:
/// mariadb/tidb → Mysql, cockroachdb/redshift/timescaledb → Postgres.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbDriver {
    Postgres,
    Mysql,
    Mssql,
    Sqlite,
}

impl DbDriver {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "postgres" | "postgresql" | "cockroachdb" | "redshift" | "timescaledb" => {
                Some(Self::Postgres)
            }
            "mysql" | "mariadb" | "tidb" => Some(Self::Mysql),
            "mssql" | "sqlserver" => Some(Self::Mssql),
            "sqlite" | "sqlite3" => Some(Self::Sqlite),
            _ => None,
        }
    }
}

/// Test/çalıştırma için çözülmüş (secret düz metin) bağlantı bilgisi.
#[derive(Debug, Clone)]
pub struct DbConfig {
    pub driver: DbDriver,
    pub mode: String, // "fields" | "uri"
    pub host: Option<String>,
    pub port: Option<i32>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub secret: Option<String>, // parola (fields) veya bağlantı dizesi (uri)
    pub options: serde_json::Value,
}

#[derive(Debug)]
pub struct DbError(pub String);
impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for DbError {}

/// fields modundan sqlx bağlantı dizesi (postgres/mysql). uri modunda secret
/// doğrudan bağlantı dizesidir.
pub(crate) fn sqlx_uri(cfg: &DbConfig, scheme: &str, default_port: i32) -> String {
    if cfg.mode == "uri" {
        return cfg.secret.clone().unwrap_or_default();
    }
    let host = cfg.host.as_deref().unwrap_or("localhost");
    let port = cfg.port.unwrap_or(default_port);
    let db = cfg.database.as_deref().unwrap_or("");
    let user = cfg.username.as_deref().unwrap_or("");
    let pass = cfg.secret.as_deref().unwrap_or("");
    format!("{scheme}://{user}:{pass}@{host}:{port}/{db}")
}

/// SQLite bağlantı dizesi: fields modunda `database` dosya yoludur.
pub(crate) fn sqlite_uri(cfg: &DbConfig) -> String {
    if cfg.mode == "uri" {
        return cfg.secret.clone().unwrap_or_default();
    }
    format!("sqlite://{}", cfg.database.as_deref().unwrap_or(""))
}

/// `:name` yer tutucularını sıralı (marker, value) listesine çevirir.
/// marker_fn(index) sürücüye göre işaret üretir ($1 / ? / @P1).
///
/// Değerler yer tutucuların METİN sırasında push edilir — `?` gibi konum-örtük
/// işaretlerde (MySQL/SQLite) zorunlu. Tek tırnaklı string literal'ler atlanır;
/// `::cast` (Postgres) parametre sayılmaz. Aynı isim tekrar geçerse her geçiş
/// yeni bir bind üretir (pg'de $1,$2 aynı değeri alır — davranış eşdeğer).
pub fn bind_params(
    query: &str,
    params: &serde_json::Map<String, serde_json::Value>,
    marker: impl Fn(usize) -> String,
) -> (String, Vec<serde_json::Value>) {
    let bytes = query.as_bytes();
    let mut sql = String::with_capacity(query.len());
    let mut bound: Vec<serde_json::Value> = Vec::new();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == '\'' {
            in_str = !in_str;
            sql.push(c);
            i += 1;
            continue;
        }
        if !in_str && c == ':' && !sql.ends_with(':') {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                if let Some(value) = params.get(&query[start..end]) {
                    bound.push(value.clone());
                    sql.push_str(&marker(bound.len()));
                    i = end;
                    continue;
                }
            }
        }
        sql.push(c);
        i += 1;
    }
    (sql, bound)
}

#[cfg(test)]
mod driver_tests {
    use super::DbDriver;

    #[test]
    fn aliases_resolve_to_wire_protocol() {
        assert_eq!(DbDriver::parse("mariadb"), Some(DbDriver::Mysql));
        assert_eq!(DbDriver::parse("tidb"), Some(DbDriver::Mysql));
        assert_eq!(DbDriver::parse("cockroachdb"), Some(DbDriver::Postgres));
        assert_eq!(DbDriver::parse("redshift"), Some(DbDriver::Postgres));
        assert_eq!(DbDriver::parse("timescaledb"), Some(DbDriver::Postgres));
        assert_eq!(DbDriver::parse("sqlserver"), Some(DbDriver::Mssql));
        assert_eq!(DbDriver::parse("sqlite"), Some(DbDriver::Sqlite));
        assert_eq!(DbDriver::parse("oracle"), None);
    }
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

    #[test]
    fn values_follow_text_order_not_map_order() {
        // Map alfabetik gezinir (a, z) ama metinde :z önce geliyor —
        // konum-örtük `?` işaretlerinde değerler metin sırasında olmalı.
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!("ilk"));
        m.insert("z".into(), json!("son"));
        let (sql, vals) = bind_params("VALUES (:z, :a)", &m, |_| "?".to_string());
        assert_eq!(sql, "VALUES (?, ?)");
        assert_eq!(vals, vec![json!("son"), json!("ilk")]);
    }

    #[test]
    fn pg_cast_and_string_literals_untouched() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!(1));
        let (sql, vals) = bind_params("SELECT ':a', x::int WHERE y = :a", &m, |i| format!("${i}"));
        assert_eq!(sql, "SELECT ':a', x::int WHERE y = $1");
        assert_eq!(vals, vec![json!(1)]);
    }

    #[test]
    fn repeated_name_binds_each_occurrence() {
        let mut m = serde_json::Map::new();
        m.insert("a".into(), json!(5));
        let (sql, vals) = bind_params("WHERE x = :a OR y = :a", &m, |_| "?".to_string());
        assert_eq!(sql, "WHERE x = ? OR y = ?");
        assert_eq!(vals, vec![json!(5), json!(5)]);
    }

    #[test]
    fn unknown_placeholder_left_as_is() {
        let m = serde_json::Map::new();
        let (sql, vals) = bind_params("WHERE x = :missing", &m, |_| "?".to_string());
        assert_eq!(sql, "WHERE x = :missing");
        assert!(vals.is_empty());
    }
}
