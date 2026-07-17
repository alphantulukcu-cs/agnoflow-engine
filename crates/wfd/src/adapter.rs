use crate::{project, repo, storage};
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

    /// Validator raporunu tek satırlık InvalidJson hatasına çevirir.
    fn validation_error(report: &validator::ValidationReport) -> crate::error::WfdError {
        let summary = report
            .errors
            .iter()
            .map(|e| format!("[{}] {}: {}", e.code, e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        crate::error::WfdError::InvalidJson(format!("validator: {summary}"))
    }

    /// project_id verilmişse tenant'a aitliğini doğrular, verilmemişse
    /// tenant'ın varsayılan projesini çözer (eski istemci uyumluluğu).
    async fn resolve_project(
        &self,
        orgtnt_id: Uuid,
        project_id: Option<Uuid>,
    ) -> Result<Uuid, crate::error::WfdError> {
        match project_id {
            Some(id) => {
                project::assert_in_tenant(&self.pool, id, orgtnt_id).await?;
                Ok(id)
            }
            None => project::resolve_default(&self.pool, orgtnt_id).await,
        }
    }

    /// Upload a new WFD — v2.2 yükleme kapısı (M14) + custom validator,
    /// sonra JSON OpenDAL'a, metadata PostgreSQL'e.
    pub async fn upload(
        &self,
        orgtnt_id: Uuid,
        project_id: Option<Uuid>,
        wfd_json: &Value,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let wfd = Wfd::from_value(wfd_json.clone())
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;

        let report = validator::validate(&wfd);
        if !report.is_valid() {
            return Err(Self::validation_error(&report));
        }

        let name = if wfd.name.trim().is_empty() {
            &wfd.id
        } else {
            &wfd.name
        };
        let project_id = self.resolve_project(orgtnt_id, project_id).await?;
        let version = repo::next_version(&self.pool, project_id, name).await?;
        let wfd_id = Uuid::new_v4();
        let key = storage::s3_key(wfd_id, version);

        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(
            &self.pool, wfd_id, orgtnt_id, project_id, name, version, &key,
            // TODO: gerçek owner auth entegrasyonundan (şimdilik admin)
            "published", None, &[], "admin", None,
        ).await?;
        Ok((wfd_id, version))
    }

    /// slug: isimden basit, güvenli bir id üretir (draft iskeleti için).
    fn slug(name: &str) -> String {
        let s: String = name.trim().to_lowercase().chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() { "wfd".into() } else { s }
    }

    /// Yeni draft oluşturur. İskelet JSON verilmezse minimal v2.2 taslağı yazılır.
    /// Validasyon YOK. Tek-draft ihlalinde WfdError::Conflict.
    pub async fn create_draft(
        &self,
        orgtnt_id:   Uuid,
        project_id:  Option<Uuid>,
        name:        &str,
        description: Option<&str>,
        tags:        &[String],
        wfd_json:    Option<&Value>,
        source_template_id: Option<Uuid>,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let project_id = self.resolve_project(orgtnt_id, project_id).await?;
        let version = repo::next_version(&self.pool, project_id, name).await?;
        let wfd_id = Uuid::new_v4();
        let key = storage::s3_key(wfd_id, version);

        let skeleton = serde_json::json!({
            "wfd_version": "2.2",
            "id": Self::slug(name),
            "name": name,
            "description": description.unwrap_or(""),
            "nodes": [],
            "transitions": [],
        });
        let doc = wfd_json.unwrap_or(&skeleton);
        let bytes = serde_json::to_vec(doc)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage.write(&key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(
            &self.pool, wfd_id, orgtnt_id, project_id, name, version, &key,
            // TODO: gerçek owner auth entegrasyonundan (şimdilik admin)
            "draft", description, tags, "admin", source_template_id,
        ).await?;
        Ok((wfd_id, version))
    }

    /// Draft'ın ham JSON'unu döner (Wfd parse ETMEZ — eksik/geçersiz olabilir).
    pub async fn fetch_draft_json(&self, wfd_id: Uuid, version: i32)
        -> Result<Value, crate::error::WfdError>
    {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil (status={})", meta.status)));
        }
        let bytes = self.storage.read(&meta.s3_key).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
    }

    /// Draft JSON + metadata'yı overwrite eder. Validasyon YOK. Cache invalidate.
    pub async fn save_draft(
        &self,
        wfd_id:      Uuid,
        version:     i32,
        wfd_json:    &Value,
        description: Option<&str>,
        tags:        Option<&[String]>,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil")));
        }
        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        // DB status-gate'i ÖNCE koş: satırın hâlâ draft olduğunu atomik doğrular.
        // Eşzamanlı bir publish araya girerse artık immutable JSON'a dokunmayız.
        repo::update_draft(&self.pool, wfd_id, version, description, tags).await?;
        self.storage.write(&meta.s3_key, bytes).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Draft'ı yayınlar: tam v2.2 validator, geçerse status='published'.
    /// Geçmezse InvalidJson(validator özeti) döner, draft kalır.
    pub async fn publish_draft(&self, wfd_id: Uuid, version: i32)
        -> Result<(), crate::error::WfdError>
    {
        let json = self.fetch_draft_json(wfd_id, version).await?;
        let wfd = Wfd::from_value(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let report = validator::validate(&wfd);
        if !report.is_valid() {
            return Err(Self::validation_error(&report));
        }
        repo::set_published(&self.pool, wfd_id, version).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Draft'ı onaya gönderir: tam v2.2 validator (yayınla ile AYNI kapı),
    /// geçerse pending_approval. Geçmezse draft kalır, hata döner.
    pub async fn submit_draft(&self, wfd_id: Uuid, version: i32, submitted_by: &str)
        -> Result<(), crate::error::WfdError>
    {
        let json = self.fetch_draft_json(wfd_id, version).await?;
        let wfd = Wfd::from_value(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let report = validator::validate(&wfd);
        if !report.is_valid() {
            return Err(Self::validation_error(&report));
        }
        repo::set_pending(&self.pool, wfd_id, version, submitted_by).await
    }

    /// Onay bekleyeni yayınlar. Validator yeniden koşar (pending JSON immutable
    /// olmalı ama savunmacı davranıyoruz); geçerse published.
    pub async fn approve_draft(&self, wfd_id: Uuid, version: i32)
        -> Result<(), crate::error::WfdError>
    {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "pending_approval" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} onay beklemiyor (status={})", meta.status)));
        }
        let bytes = self.storage.read(&meta.s3_key).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let wfd = Wfd::from_value(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let report = validator::validate(&wfd);
        if !report.is_valid() {
            return Err(Self::validation_error(&report));
        }
        repo::set_published_from_pending(&self.pool, wfd_id, version).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Onay bekleyeni reddeder: draft'a döner, gerekçe kaydedilir.
    pub async fn reject_draft(&self, wfd_id: Uuid, version: i32, note: Option<&str>)
        -> Result<(), crate::error::WfdError>
    {
        repo::set_rejected(&self.pool, wfd_id, version, note).await
    }

    /// Published bir versiyonu edit'e açar: JSON'unu kopyalayıp yeni draft (max+1) yaratır.
    pub async fn new_draft_from(&self, src_id: Uuid, src_version: i32)
        -> Result<(Uuid, i32), crate::error::WfdError>
    {
        let src = repo::get_meta_any(&self.pool, src_id, src_version).await?;
        if src.status != "published" {
            return Err(crate::error::WfdError::Conflict(
                format!("{src_id} v{src_version} published değil")));
        }
        let bytes = self.storage.read(&src.s3_key).await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.create_draft(
            src.orgtnt_id, Some(src.project_id), &src.name,
            src.description.as_deref(), &src.tags, Some(&json),
            src.source_template_id,
        ).await
    }

    /// Draft'ı iskarta eder (JSON + satır). Published dokunulmaz.
    pub async fn delete_draft(&self, wfd_id: Uuid, version: i32)
        -> Result<(), crate::error::WfdError>
    {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(
                format!("{wfd_id} v{version} draft değil")));
        }
        // DB status-gate'i ÖNCE koş; ancak satır silinirse (hâlâ draft'tı)
        // storage'ı best-effort temizle. Eşzamanlı publish JSON'u korur.
        repo::delete_draft(&self.pool, wfd_id, version).await?;
        let _ = self.storage.delete(&meta.s3_key).await;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::WfdAdapter;

    #[test]
    fn slug_lowercases_and_replaces_non_alnum() {
        assert_eq!(WfdAdapter::slug("Kredi Başvuru!"), "kredi_ba_vuru");
        assert_eq!(WfdAdapter::slug("  "), "wfd");
        assert_eq!(WfdAdapter::slug("A-B_C"), "a_b_c");
    }
}
