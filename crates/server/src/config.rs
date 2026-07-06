#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port:         u16,
    pub storage:      wf_wfd::StorageConfig,
    pub jwt_secret:   String,
    /// İzinli CORS origin'leri (virgülle ayrık). Boşsa localhost dev origin'leri.
    pub cors_origins: Vec<String>,
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
            jwt_secret: std::env::var("JWT_SECRET")
                .map_err(|_| "JWT_SECRET env var required")?,
            cors_origins: std::env::var("CORS_ORIGINS")
                .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
                .unwrap_or_else(|_| {
                    vec![
                        "http://localhost:5173".into(),
                        "http://localhost:3000".into(),
                        "http://127.0.0.1:5173".into(),
                    ]
                }),
        })
    }
}
