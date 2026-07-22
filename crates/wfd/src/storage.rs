use opendal::{services, Operator};

#[derive(Debug, Clone)]
pub struct StorageConfig {
    pub backend: StorageBackend,
    pub path: String,
    pub s3_bucket: Option<String>,
    pub s3_region: Option<String>,
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
            let builder = services::S3::default()
                .bucket(cfg.s3_bucket.as_deref().unwrap_or("wf-engine"))
                .region(cfg.s3_region.as_deref().unwrap_or("us-east-1"));
            Ok(Operator::new(builder)?.finish())
        }
    }
}

/// Canonical storage key for a WFD JSON file.
pub fn s3_key(wfd_id: uuid::Uuid, version: i32) -> String {
    format!("wfd/{wfd_id}/{version}.json")
}
