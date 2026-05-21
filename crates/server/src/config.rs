#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port:         u16,
    pub storage:      wf_wfd::StorageConfig,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        dotenvy::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL")
                .map_err(|_| "DATABASE_URL not set")?,
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .map_err(|_| "PORT must be a number")?,
            storage: wf_wfd::StorageConfig::from_env(),
        })
    }
}
