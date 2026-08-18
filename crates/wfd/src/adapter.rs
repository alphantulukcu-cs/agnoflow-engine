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
        let wfd = Wfd::from_value_checked(wfd_json.clone())
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;

        // WFC cross-WFD kuralları (girdi kümesi, tip uyumu, `$call.result.*` anahtarları,
        // döngü) çağrılan WFD'leri okumayı gerektirir. Validator SAF ve SENKRONdur, bu
        // yüzden gerekli dokümanlar önce async olarak toplanır, sonra bellekteki bir
        // katalog sync provider olarak verilir.
        let callees = self.prefetch_callees(orgtnt_id, &wfd).await;
        let report = validator::validate_with(&wfd, Some(&callees));
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
        let key = storage::s3_key(orgtnt_id, wfd_id, version);

        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(
            &self.pool,
            wfd_id,
            orgtnt_id,
            project_id,
            name,
            version,
            &key,
            // TODO: gerçek owner auth entegrasyonundan (şimdilik admin)
            "published",
            None,
            &[],
            "admin",
            None,
            // WFC: bu WFD'nin başka akışlar tarafından çağrılabilmesi için doküman
            // kimliği indekslenir (bkz. repo::resolve_doc).
            Some(wfd.id.as_str()),
            Some(wfd.version.as_str()),
        )
        .await?;
        Ok((wfd_id, version))
    }

    /// WFC: `calls` katalogundan başlayıp geçişli olarak çağrılan WFD'leri toplar.
    ///
    /// Geçişli olması ZORUNLU: döngü tespiti (`call_cycle` / `call_next_cycle`) yalnız
    /// çağrılanın kendi çağrılarını da görebilirse çalışır. Derinlik `MAX_PREFETCH` ile
    /// sınırlıdır — bozuk/çok derin bir graf upload'ı kilitlemesin; sınır aşılırsa
    /// döngü statik olarak kaçabilir ama runtime derinlik freni yine devreye girer.
    async fn prefetch_callees(&self, orgtnt_id: Uuid, root: &Wfd) -> CalleeCatalog {
        const MAX_PREFETCH: usize = 64;
        let mut out: Vec<Wfd> = Vec::new();
        let mut queue: Vec<(String, Option<String>)> = root
            .calls
            .values()
            .map(|d| (d.wfd_id.clone(), d.version.clone()))
            .collect();
        let mut seen: std::collections::HashSet<(String, Option<String>)> =
            queue.iter().cloned().collect();

        while let Some((doc_id, doc_version)) = queue.pop() {
            if out.len() >= MAX_PREFETCH {
                tracing::warn!(
                    "WFC ön-yükleme sınırı ({MAX_PREFETCH}) aşıldı — döngü tespiti eksik olabilir"
                );
                break;
            }
            let Ok(Some((id, version))) =
                repo::resolve_doc(&self.pool, orgtnt_id, &doc_id, doc_version.as_deref()).await
            else {
                continue; // çözülemeyen çağrı `call_version_not_published` ile raporlanır
            };
            let Ok(callee) = WfdStore::fetch(self, id, version).await else {
                continue;
            };
            for def in callee.calls.values() {
                let key = (def.wfd_id.clone(), def.version.clone());
                if seen.insert(key.clone()) {
                    queue.push(key);
                }
            }
            out.push(callee);
        }
        CalleeCatalog(out)
    }

    /// slug: isimden basit, güvenli bir id üretir (draft iskeleti için).
    fn slug(name: &str) -> String {
        let s: String = name
            .trim()
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let s = s.trim_matches('_').to_string();
        if s.is_empty() {
            "wfd".into()
        } else {
            s
        }
    }

    /// Yeni draft oluşturur. İskelet JSON verilmezse minimal v2.2 taslağı yazılır.
    /// Validasyon YOK. Tek-draft ihlalinde WfdError::Conflict.
    pub async fn create_draft(
        &self,
        orgtnt_id: Uuid,
        project_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        tags: &[String],
        wfd_json: Option<&Value>,
        source_template_id: Option<Uuid>,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let project_id = self.resolve_project(orgtnt_id, project_id).await?;
        let version = repo::next_version(&self.pool, project_id, name).await?;
        let wfd_id = Uuid::new_v4();
        let key = storage::s3_key(orgtnt_id, wfd_id, version);

        let skeleton = serde_json::json!({
            "wfd_version": "2.2",
            "id": Self::slug(name),
            "name": name,
            "description": description.unwrap_or(""),
            "nodes": [],
            "transitions": [],
        });
        let doc = wfd_json.unwrap_or(&skeleton);
        // WFC: doküman kimliği draft'ta da saklanır (yayınlanınca çağrılabilir olsun).
        let doc_id = doc.get("id").and_then(Value::as_str);
        let doc_version = doc.get("version").and_then(Value::as_str);
        let bytes = serde_json::to_vec(doc)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;

        repo::insert(
            &self.pool,
            wfd_id,
            orgtnt_id,
            project_id,
            name,
            version,
            &key,
            // TODO: gerçek owner auth entegrasyonundan (şimdilik admin)
            "draft",
            description,
            tags,
            "admin",
            source_template_id,
            // Draft satır da doküman kimliğini taşır; `resolve_doc` yalnız
            // `status='published'` satırları döndürdüğü için draft çağrılamaz.
            doc_id,
            doc_version,
        )
        .await?;
        Ok((wfd_id, version))
    }

    /// Draft'ın ham JSON'unu döner (Wfd parse ETMEZ — eksik/geçersiz olabilir).
    pub async fn fetch_draft_json(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Value, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(format!(
                "{wfd_id} v{version} draft değil (status={})",
                meta.status
            )));
        }
        let bytes = self
            .storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
    }

    /// Draft JSON + metadata'yı overwrite eder. Validasyon YOK. Cache invalidate.
    pub async fn save_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        wfd_json: &Value,
        description: Option<&str>,
        tags: Option<&[String]>,
        // T‑B4: kilit sahibinin kimliği — kapı repo::update_draft'ın WHERE'inde.
        lock_user_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(format!(
                "{wfd_id} v{version} draft değil"
            )));
        }
        let bytes = serde_json::to_vec(wfd_json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        // DB status-gate'i ÖNCE koş: satırın hâlâ draft olduğunu atomik doğrular.
        // Eşzamanlı bir publish araya girerse artık immutable JSON'a dokunmayız.
        repo::update_draft(&self.pool, wfd_id, version, description, tags, lock_user_id).await?;
        self.storage
            .write(&meta.s3_key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    // ── T‑B4: taslak kilidi ────────────────────────────────────────────────
    // İnce sarmalayıcılar: kilit mantığı SQL'de (repo), burada yalnız yönlendirme.

    /// Kilidi alır; güncel meta (kilit alanları dahil) döner. Kilit zaten bizdeyse
    /// çağrı etkisizdir — kilidin süresi yok, tazelenecek bir şey de yok.
    pub async fn lock_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        orgtnt_id: Uuid,
        user_id: Uuid,
    ) -> Result<crate::models::WfdMeta, crate::error::WfdError> {
        repo::acquire_lock(&self.pool, wfd_id, version, orgtnt_id, user_id).await
    }

    /// Kilidi sahibinden bağımsız düşürür — yetki kararı ROTADA verilir (proje/tenant
    /// admini). Bkz. `repo::force_release_lock`.
    pub async fn force_unlock_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        orgtnt_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        repo::force_release_lock(&self.pool, wfd_id, version, orgtnt_id).await
    }

    /// Kilidi bırakır — yalnız sahibi.
    pub async fn unlock_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        orgtnt_id: Uuid,
        user_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        repo::release_lock(&self.pool, wfd_id, version, orgtnt_id, user_id).await
    }

    /// Draft/pending bir WFD'yi **upload ile AYNI kapıdan** doğrular.
    ///
    /// Neden ayrı bir yardımcı: `publish`/`submit`/`approve` yolları resolver'sız
    /// `validate()` çağırıyordu, yani WFC'nin cross-WFD kuralları (çağrılanın var
    /// olması, girdi sözleşmesi, tip uyumu, döngü) BU YOLLARDA HİÇ KOŞMUYORDU.
    /// Sonuç: çağıran akış, çağrılan henüz yayınlanmamışken sessizce publish
    /// edilebiliyordu ve hata ancak ÇALIŞMA ANINDA `WFD.CallNotFound` olarak
    /// ortaya çıkıyordu — `wait` modunda WFE o node'da sonsuza kadar bekler.
    async fn validate_for_release(
        &self,
        wfd_id: Uuid,
        version: i32,
        wfd: &Wfd,
    ) -> Result<(), crate::error::WfdError> {
        // Versiyon ZORUNLU: `get_meta_any` (wfd_id, version) ile arar. Sabit 0
        // geçilirse satır bulunamaz, tenant None olur ve cross-WFD kuralları
        // sessizce atlanır — yani kapı hiç kapanmaz.
        let orgtnt_id = repo::get_meta_any(&self.pool, wfd_id, version)
            .await
            .map(|m| m.orgtnt_id)
            .ok();
        let report = match orgtnt_id {
            Some(tid) => {
                let callees = self.prefetch_callees(tid, wfd).await;
                validator::validate_with(wfd, Some(&callees))
            }
            // Tenant çözülemezse cross-WFD kurallarını atlamak yerine yerel
            // doğrulama ile devam et — sessizce geçirmekten iyidir, ama bu yol
            // pratikte oluşmaz (satır zaten okundu).
            None => validator::validate(wfd),
        };
        if !report.is_valid() {
            return Err(Self::validation_error(&report));
        }
        Ok(())
    }

    /// Draft'ı yayınlar: tam v2.2 validator, geçerse status='published'.
    /// Geçmezse InvalidJson(validator özeti) döner, draft kalır.
    pub async fn publish_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        // T‑B4: yayınlamak da kilit ister — A'nın yarım işi B tarafından yayınlanmasın.
        lock_user_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        // Kilit ÖNCE sorulur: yetkisi olmayana içerik hatası göstermek yanlış sırayı
        // öğretir ("JSON'u düzelt" der, oysa sorun yetkidir). Asıl kapı set_published'ın
        // WHERE'inde kalır — bu yalnız hata sırası.
        repo::assert_lock_held(&self.pool, wfd_id, version, lock_user_id).await?;
        let json = self.fetch_draft_json(wfd_id, version).await?;
        let wfd = Wfd::from_value_checked(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.validate_for_release(wfd_id, version, &wfd).await?;
        repo::set_published(&self.pool, wfd_id, version, lock_user_id).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Draft'ı onaya gönderir: tam v2.2 validator (yayınla ile AYNI kapı),
    /// geçerse pending_approval. Geçmezse draft kalır, hata döner.
    pub async fn submit_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        submitted_by: &str,
        lock_user_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        repo::assert_lock_held(&self.pool, wfd_id, version, lock_user_id).await?;
        let json = self.fetch_draft_json(wfd_id, version).await?;
        let wfd = Wfd::from_value_checked(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.validate_for_release(wfd_id, version, &wfd).await?;
        repo::set_pending(&self.pool, wfd_id, version, submitted_by, lock_user_id).await
    }

    /// Onay bekleyeni yayınlar. Validator yeniden koşar (pending JSON immutable
    /// olmalı ama savunmacı davranıyoruz); geçerse published.
    pub async fn approve_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "pending_approval" {
            return Err(crate::error::WfdError::Conflict(format!(
                "{wfd_id} v{version} onay beklemiyor (status={})",
                meta.status
            )));
        }
        let bytes = self
            .storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let wfd = Wfd::from_value_checked(json)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        // Onay yolu da aynı kapıdan geçer: pending JSON immutable olsa bile
        // ÇAĞRILAN akışlar bu arada değişmiş/silinmiş olabilir.
        self.validate_for_release(wfd_id, version, &wfd).await?;
        repo::set_published_from_pending(&self.pool, wfd_id, version).await?;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }

    /// Onay bekleyeni reddeder: draft'a döner, gerekçe kaydedilir.
    pub async fn reject_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        note: Option<&str>,
    ) -> Result<(), crate::error::WfdError> {
        repo::set_rejected(&self.pool, wfd_id, version, note).await
    }

    /// Published bir versiyonu edit'e açar: JSON'unu kopyalayıp yeni draft (max+1) yaratır.
    pub async fn new_draft_from(
        &self,
        src_id: Uuid,
        src_version: i32,
    ) -> Result<(Uuid, i32), crate::error::WfdError> {
        let src = repo::get_meta_any(&self.pool, src_id, src_version).await?;
        if src.status != "published" {
            return Err(crate::error::WfdError::Conflict(format!(
                "{src_id} v{src_version} published değil"
            )));
        }
        let bytes = self
            .storage
            .read(&src.s3_key)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        let json: Value = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        let created = self
            .create_draft(
                src.orgtnt_id,
                Some(src.project_id),
                &src.name,
                src.description.as_deref(),
                &src.tags,
                Some(&json),
                src.source_template_id,
            )
            .await?;
        // Editör layout'unu (varsa) yeni drafta taşı — step id'leri import-id anahtarlı
        // ve versiyondan bağımsız olduğundan blob aynen kopyalanır (best-effort).
        if let Ok(Some(layout)) = self.fetch_layout(src_id, src_version).await {
            let _ = self.save_layout(created.0, created.1, &layout).await;
        }
        // Senaryolar (kaydedilmiş simülasyon koşuları) da yeni drafta taşınır:
        // yeni versiyon eskinin regresyon testleriyle karşılanmalı (best-effort).
        if let Ok(Some(scenarios)) = self.fetch_scenarios(src_id, src_version).await {
            let _ = self.save_scenarios(created.0, created.1, &scenarios).await;
        }
        Ok(created)
    }

    /// Editör layout companion'ını (opaque JSON) yazar. Şema-VALID doküman değildir;
    /// parse/validate YOK. Versiyonun var olduğunu doğrular (herhangi status).
    pub async fn save_layout(
        &self,
        wfd_id: Uuid,
        version: i32,
        layout: &Value,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::layout_key(meta.orgtnt_id, wfd_id, version);
        let bytes = serde_json::to_vec(layout)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Layout companion'ını döner; blob yoksa None (hata değil).
    pub async fn fetch_layout(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Option<Value>, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::layout_key(meta.orgtnt_id, wfd_id, version);
        let parse = |buf: opendal::Buffer| -> Result<Value, crate::error::WfdError> {
            serde_json::from_slice(&buf.to_bytes())
                .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
        };
        match self.storage.read(&key).await {
            Ok(buf) => Ok(Some(parse(buf)?)),
            // Tenant prefix'ine geçmeden ÖNCE yazılmış layout'lar eski anahtarda
            // kalır (layout anahtarı DB'de saklanmaz). Geriye dönük oku.
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => {
                let legacy = storage::legacy_layout_key(wfd_id, version);
                match self.storage.read(&legacy).await {
                    Ok(buf) => Ok(Some(parse(buf)?)),
                    Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(None),
                    Err(e) => Err(crate::error::WfdError::Storage(e.to_string())),
                }
            }
            Err(e) => Err(crate::error::WfdError::Storage(e.to_string())),
        }
    }

    /// Versiyonun HAM JSON'unu döner — status'e bakmaz. `fetch_draft_json`
    /// yalnız draft'a izin verir, `fetch` ise parse edilmiş `Wfd` döner;
    /// senaryo koşucusuna ise `terminals[]` kataloğunu okuyabilmek için ham
    /// belge lazım (bkz. `wf_wfe::scenario::infer_terminal_id`).
    pub async fn fetch_json_any(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Value, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let bytes = self
            .storage
            .read(&meta.s3_key)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?
            .to_bytes();
        serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))
    }

    /// Senaryo sidecar'ını (opaque JSON) yazar. Şema-VALID doküman DEĞİLDİR;
    /// parse/validate YOK — şekli `wf_wfe::scenario::ScenarioSet` tarafında
    /// doğrulanır. Versiyonun var olduğunu doğrular (herhangi status: yayınlanmış
    /// akışa test eklemek akışı değiştirmez).
    pub async fn save_scenarios(
        &self,
        wfd_id: Uuid,
        version: i32,
        scenarios: &Value,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::scenarios_key(meta.orgtnt_id, wfd_id, version);
        let bytes = serde_json::to_vec(scenarios)
            .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?;
        self.storage
            .write(&key, bytes)
            .await
            .map_err(|e| crate::error::WfdError::Storage(e.to_string()))?;
        Ok(())
    }

    /// Senaryo sidecar'ını döner; blob yoksa None (hata değil — henüz senaryo
    /// yazılmamış WFD'ler bu yoldan geçer).
    pub async fn fetch_scenarios(
        &self,
        wfd_id: Uuid,
        version: i32,
    ) -> Result<Option<Value>, crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        let key = storage::scenarios_key(meta.orgtnt_id, wfd_id, version);
        match self.storage.read(&key).await {
            Ok(buf) => Ok(Some(
                serde_json::from_slice(&buf.to_bytes())
                    .map_err(|e| crate::error::WfdError::InvalidJson(e.to_string()))?,
            )),
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(crate::error::WfdError::Storage(e.to_string())),
        }
    }

    /// Draft'ı iskarta eder (JSON + satır). Published dokunulmaz.
    pub async fn delete_draft(
        &self,
        wfd_id: Uuid,
        version: i32,
        lock_user_id: Uuid,
    ) -> Result<(), crate::error::WfdError> {
        let meta = repo::get_meta_any(&self.pool, wfd_id, version).await?;
        if meta.status != "draft" {
            return Err(crate::error::WfdError::Conflict(format!(
                "{wfd_id} v{version} draft değil"
            )));
        }
        // DB status-gate'i ÖNCE koş; ancak satır silinirse (hâlâ draft'tı)
        // storage'ı best-effort temizle. Eşzamanlı publish JSON'u korur.
        repo::delete_draft(&self.pool, wfd_id, version, lock_user_id).await?;
        let _ = self.storage.delete(&meta.s3_key).await;
        // Sidecar'lar da gider — aksi halde storage'da öksüz blob birikir.
        // (Layout bugüne kadar temizlenmiyordu; senaryo sidecar'ını eklerken
        // yanına ikinci bir öksüz bırakmak anlamsız olurdu.)
        let _ = self
            .storage
            .delete(&storage::layout_key(meta.orgtnt_id, wfd_id, version))
            .await;
        let _ = self
            .storage
            .delete(&storage::scenarios_key(meta.orgtnt_id, wfd_id, version))
            .await;
        self.cache.write().await.remove(&(wfd_id, version));
        Ok(())
    }
}

#[async_trait]
impl WfdStore for WfdAdapter {
    /// WFC: doküman kimliğinden yayınlanmış satırı çözer (`wfd_meta.doc_id` indeksi).
    async fn resolve_doc(
        &self,
        orgtnt_id: Uuid,
        doc_id: &str,
        doc_version: Option<&str>,
    ) -> Result<Option<(Uuid, i32)>, EngineError> {
        repo::resolve_doc(&self.pool, orgtnt_id, doc_id, doc_version)
            .await
            .map_err(|e| EngineError::WfdPort(e.to_string()))
    }

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
        let wfd = Wfd::from_json_checked(text)?;

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

/// Ön-yüklenmiş çağrılan WFD'ler — validator'ın SENKRON `WfdProvider`'ı.
///
/// `version: None` = "en son yayınlanmış": burada katalogdaki tek sürüm o sürümdür
/// (ön-yükleme `resolve_doc` ile zaten en sonu seçti).
struct CalleeCatalog(Vec<Wfd>);

impl validator::WfdProvider for CalleeCatalog {
    fn resolve(&self, wfd_id: &str, version: Option<&str>) -> Option<Wfd> {
        self.0
            .iter()
            .find(|w| w.id == wfd_id && version.map_or(true, |v| w.version == v))
            .cloned()
    }

}