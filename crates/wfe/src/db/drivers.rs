//! Sürücü-özel bağlantı testi. fields modu bileşenlerden URI kurar; uri modu
//! secret'ı doğrudan bağlantı dizesi olarak kullanır.
use super::{DbConfig, DbDriver, DbError};

fn field<'a>(o: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    o.get(k).and_then(|v| v.as_str())
}

/// fields modundan sqlx bağlantı dizesi (postgres/mysql).
fn sqlx_uri(cfg: &DbConfig, scheme: &str, default_port: i32) -> String {
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

pub async fn test(cfg: &DbConfig) -> Result<(), DbError> {
    match cfg.driver {
        DbDriver::Postgres => test_sqlx_pg(cfg).await,
        DbDriver::Mysql => test_sqlx_my(cfg).await,
        DbDriver::Mssql => test_mssql(cfg).await,
    }
}

async fn test_sqlx_pg(cfg: &DbConfig) -> Result<(), DbError> {
    use sqlx::postgres::PgPoolOptions;
    let uri = sqlx_uri(cfg, "postgres", 5432);
    let pool = PgPoolOptions::new().max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(8))
        .connect(&uri).await.map_err(|e| DbError(e.to_string()))?;
    sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| DbError(e.to_string()))?;
    pool.close().await;
    Ok(())
}

async fn test_sqlx_my(cfg: &DbConfig) -> Result<(), DbError> {
    use sqlx::mysql::MySqlPoolOptions;
    let uri = sqlx_uri(cfg, "mysql", 3306);
    let pool = MySqlPoolOptions::new().max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(8))
        .connect(&uri).await.map_err(|e| DbError(e.to_string()))?;
    sqlx::query("SELECT 1").execute(&pool).await.map_err(|e| DbError(e.to_string()))?;
    pool.close().await;
    Ok(())
}

async fn test_mssql(cfg: &DbConfig) -> Result<(), DbError> {
    use tiberius::{Config, AuthMethod};
    use tokio::net::TcpStream;
    use tokio_util::compat::TokioAsyncWriteCompatExt;

    let mut config = Config::new();
    if cfg.mode == "uri" {
        config = Config::from_ado_string(cfg.secret.as_deref().unwrap_or(""))
            .map_err(|e| DbError(e.to_string()))?;
    } else {
        config.host(cfg.host.as_deref().unwrap_or("localhost"));
        config.port(cfg.port.unwrap_or(1433) as u16);
        if let Some(db) = &cfg.database { config.database(db); }
        config.authentication(AuthMethod::sql_server(
            cfg.username.as_deref().unwrap_or(""),
            cfg.secret.as_deref().unwrap_or(""),
        ));
        if field(&cfg.options, "encrypt") == Some("false") {
            config.encryption(tiberius::EncryptionLevel::NotSupported);
        } else {
            config.trust_cert();
        }
    }
    let tcp = TcpStream::connect(config.get_addr()).await.map_err(|e| DbError(e.to_string()))?;
    tcp.set_nodelay(true).ok();
    let mut client = tiberius::Client::connect(config, tcp.compat_write()).await
        .map_err(|e| DbError(e.to_string()))?;
    client.simple_query("SELECT 1").await.map_err(|e| DbError(e.to_string()))?;
    Ok(())
}
