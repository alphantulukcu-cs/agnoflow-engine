use opendal::{services, Operator};

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    pub path: String,
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
    /// S3 endpoint URL. AWS için boş bırakılır; S3-uyumlu depolar
    /// (Garage, MinIO) için zorunlu — ör. `http://garage.test.cs.com.tr:3900`.
    pub s3_endpoint: Option<String>,
    /// S3 access key id. Verilmezse OpenDAL ortam/config zincirine düşer.
    pub s3_access_key_id: Option<String>,
    /// S3 secret access key.
    pub s3_secret_access_key: Option<String>,
}

#[derive(Debug, Clone)]
pub enum StorageBackend {
    Local,
    S3,
}

impl StorageConfig {
    pub fn from_env() -> Self {
        let backend = match std::env::var("STORAGE_BACKEND")
            .unwrap_or_else(|_| "local".into())
            .as_str()
        {
            "s3" => StorageBackend::S3,
            _ => StorageBackend::Local,
        };
        Self {
            backend,
            path: std::env::var("STORAGE_PATH").unwrap_or_else(|_| "./storage".into()),
            s3_bucket: std::env::var("STORAGE_S3_BUCKET").ok(),
            s3_region: std::env::var("STORAGE_S3_REGION").ok(),
            s3_endpoint: std::env::var("STORAGE_S3_ENDPOINT").ok(),
            s3_access_key_id: std::env::var("STORAGE_S3_ACCESS_KEY_ID").ok(),
            s3_secret_access_key: std::env::var("STORAGE_S3_SECRET_ACCESS_KEY").ok(),
        }
    }
}

pub fn build_operator(cfg: &StorageConfig) -> Result<Operator, opendal::Error> {
    match cfg.backend {
        StorageBackend::Local => {
            let builder = services::Fs::default().root(&cfg.path);
            Ok(Operator::new(builder)?.finish())
        }
        StorageBackend::S3 => {
            let mut builder = services::S3::default()
                .bucket(cfg.s3_bucket.as_deref().unwrap_or("wf-engine"))
                .region(cfg.s3_region.as_deref().unwrap_or("us-east-1"));
            // S3-uyumlu depolar (Garage, MinIO): özel endpoint + statik credential.
            // Endpoint verildiğinde ortam/EC2 metadata credential zincirini de kapatırız
            // ki makinedeki ambient AWS ayarları sızmasın (path-style default'ta kalır).
            if let Some(ep) = cfg.s3_endpoint.as_deref() {
                builder = builder.endpoint(ep).disable_config_load().disable_ec2_metadata();
            }
            if let Some(id) = cfg.s3_access_key_id.as_deref() {
                builder = builder.access_key_id(id);
            }
            if let Some(secret) = cfg.s3_secret_access_key.as_deref() {
                builder = builder.secret_access_key(secret);
            }
            Ok(Operator::new(builder)?.finish())
        }
    }
}

/// Canonical storage key for a WFD JSON file.
///
/// Multi-tenant izolasyon: her tenant kendi kök dizini altında tutulur
/// (`{orgtnt_id}/wfd/{wfd_id}/{version}.json`). Böylece S3/FS düzeyinde
/// prefix bazlı tenant ayrımı (IAM policy, listeleme, silme) mümkün olur.
pub fn s3_key(orgtnt_id: uuid::Uuid, wfd_id: uuid::Uuid, version: i32) -> String {
    format!("{orgtnt_id}/wfd/{wfd_id}/{version}.json")
}

/// Editör layout companion'ının storage anahtarı — şema-VALID WFD dokümanından AYRI
/// opaque JSON (node pozisyonları + edge path'leri + reject/collapse bayrakları). Engine
/// dokümanı `additionalProperties:false` olduğu için layout burada, yanında saklanır.
/// WFD JSON ile aynı tenant kökü altındadır.
pub fn layout_key(orgtnt_id: uuid::Uuid, wfd_id: uuid::Uuid, version: i32) -> String {
    format!("{orgtnt_id}/wfd/{wfd_id}/{version}.layout.json")
}

/// Tenant-öncesi (eski, tek-tenant) layout anahtarı. Layout anahtarı DB'de
/// SAKLANMADIĞINDAN türetilir; tenant prefix'ine geçişte eski bloblar bu
/// anahtarda kalır. `fetch_layout` yeni anahtar bulunamazsa buna düşer.
pub fn legacy_layout_key(wfd_id: uuid::Uuid, version: i32) -> String {
    format!("wfd/{wfd_id}/{version}.layout.json")
}
