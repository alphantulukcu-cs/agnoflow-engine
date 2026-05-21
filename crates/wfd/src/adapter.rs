use async_trait::async_trait;
use opendal::Operator;
use sqlx::PgPool;
use uuid::Uuid;
use wfe_core::{EngineError, WfdPort, types::wfd::WFD};
use crate::{repo, storage};

pub struct WfdAdapter {
    pub pool:    PgPool,
    pub storage: Operator,
}

impl WfdAdapter {
    pub fn new(pool: PgPool, storage: Operator) -> Self {
        Self { pool, storage }
    }

    /// Upload a new WFD — stores JSON in OpenDAL, metadata in PostgreSQL.
    pub async fn upload(
        &self,
        orgtnt_id: Uuid,
        wfd:       &WFD,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let version = repo::next_version(&self.pool, orgtnt_id, &wfd.name).await?;
        let wfd_id  = Uuid::new_v4();
        let key     = storage::s3_key(wfd_id, version);

        let bytes = serde_json::to_vec(wfd)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage.write(&key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(&self.pool, wfd_id, orgtnt_id, &wfd.name, version, &key).await?;
        Ok((wfd_id, version))
    }
}

#[async_trait]
impl WfdPort for WfdAdapter {
    async fn fetch(&self, wfd_id: Uuid, version: u32) -> Result<WFD, EngineError> {
        let meta = repo::get_meta(&self.pool, wfd_id, version as i32)
            .await
            .map_err(|e| EngineError::WfdPort(e.to_string()))?;

        let bytes = self.storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| EngineError::WfdPort(format!("storage read: {e}")))?
            .to_bytes();

        serde_json::from_slice::<WFD>(&bytes)
            .map_err(|e| EngineError::InvalidWfd(e.to_string()))
    }
}
