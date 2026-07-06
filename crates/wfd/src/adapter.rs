use crate::{repo, storage};
use async_trait::async_trait;
use opendal::Operator;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::ports::WfdStore;
use wfe_core::validator;
use wfe_core::EngineError;

pub struct WfdAdapter {
    pub pool: PgPool,
    pub storage: Operator,
    /// (wfd_id, version) → Wfd — WFD satırları immutable olduğundan
    /// süresiz cache güvenlidir (WOR-17). Kaba sınır: CACHE_CAP.
    cache: tokio::sync::RwLock<std::collections::HashMap<(Uuid, i32), Wfd>>,
}

const CACHE_CAP: usize = 256;

impl WfdAdapter {
    pub fn new(pool: PgPool, storage: Operator) -> Self {
        Self {
            pool,
            storage,
            cache: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Upload a new WFD — v2.2 yükleme kapısı (M14) + custom validator,
    /// sonra JSON OpenDAL'a, metadata PostgreSQL'e.
    pub async fn upload(
        &self,
        orgtnt_id: Uuid,
        wfd_json: &Value,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let wfd = Wfd::from_value(wfd_json.clone())
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;

        let report = validator::validate(&wfd);
        if !report.is_valid() {
            let summary = report
                .errors
                .iter()
                .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(crate::error::WfdError::InvalidJson(format!(
                "validator: {summary}"
            )));
        }

        let name = if wfd.name.trim().is_empty() {
            &wfd.id
        } else {
            &wfd.name
        };
        let version = repo::next_version(&self.pool, orgtnt_id, name).await?;
        let wfd_id = Uuid::new_v4();
        let key = storage::s3_key(wfd_id, version);

        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(&self.pool, wfd_id, orgtnt_id, name, version, &key).await?;
        Ok((wfd_id, version))
    }
}

#[async_trait]
impl WfdStore for WfdAdapter {
    async fn fetch(&self, wfd_id: Uuid, version: i32) -> Result<Wfd, EngineError> {
        if let Some(cached) = self.cache.read().await.get(&(wfd_id, version)) {
            return Ok(cached.clone());
        }
        let meta = repo::get_meta(&self.pool, wfd_id, version)
            .await
            .map_err(|e| EngineError::WfdPort(e.to_string()))?;

        let bytes = self
            .storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| EngineError::WfdPort(format!("storage read: {e}")))?
            .to_bytes();

        let text = std::str::from_utf8(&bytes)
            .map_err(|e| EngineError::InvalidWfd(format!("utf8: {e}")))?;
        // M14: wfd_version kapısı fetch'te de uygulanır — eski format çalıştırılamaz
        let wfd = Wfd::from_json(text)?;

        let mut cache = self.cache.write().await;
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert((wfd_id, version), wfd.clone());
        Ok(wfd)
    }
}
