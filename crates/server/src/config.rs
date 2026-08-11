#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: String,
    pub port: u16,
    pub storage: wf_wfd::StorageConfig,
    /// Ek-belge (attachment) depolaması — WFD JSON storage'ından AYRI konum.
    /// Varsayılan yerel yol `../work-pool-portal/storage` (engine cwd'sine göre);
    /// dış UI'ın yüklediği dosyalar burada tutulur. Engine core buna bağımlı değil,
    /// yalnız portal katmanı kullanır. `ATTACHMENT_STORAGE_*` env ile yapılandırılır.
    pub attachment_storage: wf_wfd::StorageConfig,
    pub jwt_secret: String,
    /// İzinli CORS origin'leri (virgülle ayrık). Boşsa localhost dev origin'leri.
    pub cors_origins: Vec<String>,
    /// /org admin API'si için zorunlu anahtar (X-Admin-Key). Yoksa dev modu:
    /// koruma kapalı, startup'ta uyarı loglanır (WOR-10).
    pub admin_api_key: Option<String>,
    /// Swagger UI + `/api-docs/openapi.json` mount edilsin mi. Default açık;
    /// `ENABLE_SWAGGER=false|0|off` ile (örn. prod'da) kapatılır.
    pub enable_swagger: bool,
    /// Ek-belge taşıyan alt ağaçların (`/wfe`, `/portal`) istek gövdesi üst sınırı
    /// (MB). Axum'un varsayılanı 2 MB'tır ve `body: Bytes` extractor'ı bunu WFD
    /// katalog denetiminden (`max_size_mb`) ÖNCE uygular — kataloğun izin verdiği
    /// 20 MB'lık bir slot pratikte ~2 MB'ta 413 verirdi. `ATTACHMENT_MAX_REQUEST_MB`
    /// ile yapılandırılır, varsayılan 200 (birden çok ek-belgeyi tek istekte
    /// taşıyabilecek kadar geniş). Bilerek GLOBAL uygulanmaz: diğer uçların 2 MB
    /// korumasını genişletmek gereksiz saldırı yüzeyi açardı.
    pub attachment_max_request_mb: u64,
    /// Tek istekli başlatmada çift WFE koruması (K6) — aynı parmak izinin "taze"
    /// sayıldığı pencere. `WFE_START_DEDUPE_WINDOW_SECS`, varsayılan 60 sn: bu süre
    /// içindeki tekrar aynı sonucu (`Idempotent-Replay`) alır, aşan istek yeni bir
    /// başlatma sayılır (bkz. `start_dedupe.rs`).
    pub dedupe_window_secs: u64,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        dotenvy::dotenv().ok();
        Ok(Self {
            database_url: std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL not set")?,
            port: std::env::var("PORT")
                .unwrap_or_else(|_| "3000".into())
                .parse()
                .map_err(|_| "PORT must be a number")?,
            storage: wf_wfd::StorageConfig::from_env(),
            attachment_storage: attachment_storage_from_env(),
            jwt_secret: std::env::var("JWT_SECRET").map_err(|_| "JWT_SECRET env var required")?,
            cors_origins: std::env::var("CORS_ORIGINS")
                .map(|s| s.split(',').map(|o| o.trim().to_string()).collect())
                .unwrap_or_else(|_| {
                    vec![
                        "http://localhost:5173".into(),
                        "http://localhost:3000".into(),
                        "http://127.0.0.1:5173".into(),
                    ]
                }),
            admin_api_key: std::env::var("ADMIN_API_KEY").ok(),
            enable_swagger: std::env::var("ENABLE_SWAGGER")
                .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "0" | "off"))
                .unwrap_or(true),
            attachment_max_request_mb: std::env::var("ATTACHMENT_MAX_REQUEST_MB")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200),
            dedupe_window_secs: std::env::var("WFE_START_DEDUPE_WINDOW_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(60),
        })
    }
}

/// Attachment storage config — `ATTACHMENT_STORAGE_*` env'inden okunur. WFD
/// storage'ından ayrıdır; local backend'de varsayılan kök `../work-pool-portal/storage`.
fn attachment_storage_from_env() -> wf_wfd::StorageConfig {
    let backend = match std::env::var("ATTACHMENT_STORAGE_BACKEND")
        .unwrap_or_else(|_| "local".into())
        .as_str()
    {
        "s3" => wf_wfd::StorageBackend::S3,
        _ => wf_wfd::StorageBackend::Local,
    };
    wf_wfd::StorageConfig {
        backend,
        path: std::env::var("ATTACHMENT_STORAGE_PATH")
            .unwrap_or_else(|_| "../work-pool-portal/storage".into()),
        s3_bucket: std::env::var("ATTACHMENT_STORAGE_S3_BUCKET").ok(),
        s3_region: std::env::var("ATTACHMENT_STORAGE_S3_REGION").ok(),
        s3_endpoint: std::env::var("ATTACHMENT_STORAGE_S3_ENDPOINT").ok(),
        s3_access_key_id: std::env::var("ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID").ok(),
        s3_secret_access_key: std::env::var("ATTACHMENT_STORAGE_S3_SECRET_ACCESS_KEY").ok(),
    }
}
