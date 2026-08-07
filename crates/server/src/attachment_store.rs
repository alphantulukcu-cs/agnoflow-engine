//! Ek-belge deposunun WFD BAŞINA çözümü (2026-08-07).
//!
//! Depo bugüne kadar tek bir deployment ayarıydı (`ATTACHMENT_STORAGE_*`): tüm tenant'lar,
//! tüm akışlar aynı bucket'a yazardı. Kurumsal gereksinim bunun tersi: bir akışın belgeleri
//! müşterinin kendi S3'ünde, bir diğerininki yerel diskte durabilmeli — ve bu, ortama göre
//! de değişebilmeli (test/prod ayrı bucket).
//!
//! Karar: konfigürasyon WFD DOKÜMANINA GİRMEZ, `$env` mekanizmasından okunur
//! (`wf.wfd_env_var`, sahiplik `(project_id, wfd_name)`). Gerekçe zaten `$env`in var oluş
//! gerekçesidir: doküman `(wfd_id, version)` bazında immutable'dır, prod bucket'ı değişince
//! yeni versiyon yayınlamak gerekmemeli. Secret'lar (S3 anahtarları) DB'de şifreli durur
//! ve yalnız burada çözülür — ZEN/effects onları göremez (`PublicEnv`).
//!
//! Anahtar İSİMLERİ sözleşmedir (WFD ayarları ekranında bu adlarla girilir):
//!
//! | Anahtar | Anlam |
//! |---|---|
//! | `ATTACHMENT_STORAGE_BACKEND` | `local` \| `s3`. Yoksa deployment varsayılanı kullanılır. |
//! | `ATTACHMENT_STORAGE_PATH` | local kök dizin |
//! | `ATTACHMENT_STORAGE_S3_BUCKET` / `_S3_REGION` / `_S3_ENDPOINT` | S3 hedefi |
//! | `ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID` / `_S3_SECRET_ACCESS_KEY` | kimlik (secret olarak girilir) |
//!
//! Hiçbiri tanımlı değilse deployment varsayılanına düşülür — mevcut akışlar etkilenmez.
//!
//! **Operator önbelleklenir.** S3 istemcisi kurmak her istekte yapılacak bir iş değildir;
//! anahtar, çözülmüş konfigürasyonun kendisidir (aynı config → aynı Operator). Konfigürasyon
//! değişince anahtar da değişir, eski girdi kullanılmaz kalır.

use crate::{attachments::AttachmentStore, error::AppError, state::AppState};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;
use wf_wfd::{StorageBackend, StorageConfig};
use wfe_core::v22::env::{EnvLookup, RunEnv};

/// Çözülmüş konfigürasyonun önbellek anahtarı (Operator kurulumunu belirleyen her alan).
type CacheKey = (String, String, String, String, String, String, String);

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<AttachmentStore>>> {
    static CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<AttachmentStore>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cache_key(cfg: &StorageConfig) -> CacheKey {
    (
        match cfg.backend {
            StorageBackend::Local => "local".into(),
            StorageBackend::S3 => "s3".into(),
        },
        cfg.path.clone(),
        cfg.s3_bucket.clone().unwrap_or_default(),
        cfg.s3_region.clone().unwrap_or_default(),
        cfg.s3_endpoint.clone().unwrap_or_default(),
        cfg.s3_access_key_id.clone().unwrap_or_default(),
        cfg.s3_secret_access_key.clone().unwrap_or_default(),
    )
}

/// `$env`ten tek bir anahtarı metin olarak okur. Secret'lar dahildir (bu katman
/// `RunEnv::full()` kullanır — bkz. modül başlığı).
fn env_str(env: &RunEnv, key: &str) -> Option<String> {
    let value = &env.full().lookup(key)?.value;
    match value {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::String(_) => None,
        other => Some(other.to_string()),
    }
}

/// WFD'nin ortam değişkenlerinden depo konfigürasyonu üretir. `BACKEND` tanımlı değilse
/// `None` → çağıran deployment varsayılanını kullanır.
fn config_from_env(env: &RunEnv, fallback_path: &str) -> Option<StorageConfig> {
    let backend = match env_str(env, "ATTACHMENT_STORAGE_BACKEND")?.to_ascii_lowercase().as_str() {
        "s3" => StorageBackend::S3,
        "local" => StorageBackend::Local,
        // Tanınmayan değer sessizce local'a düşmez: yanlış yazılmış bir "S£" yüzünden
        // belgelerin müşterinin bucket'ı yerine sunucu diskine yazılması, fark edilmesi
        // en zor hata sınıfıdır. Konfigürasyon yok sayılır ve varsayılana dönülür.
        _ => return None,
    };
    Some(StorageConfig {
        backend,
        path: env_str(env, "ATTACHMENT_STORAGE_PATH").unwrap_or_else(|| fallback_path.to_string()),
        s3_bucket: env_str(env, "ATTACHMENT_STORAGE_S3_BUCKET"),
        s3_region: env_str(env, "ATTACHMENT_STORAGE_S3_REGION"),
        s3_endpoint: env_str(env, "ATTACHMENT_STORAGE_S3_ENDPOINT"),
        s3_access_key_id: env_str(env, "ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID"),
        s3_secret_access_key: env_str(env, "ATTACHMENT_STORAGE_S3_SECRET_ACCESS_KEY"),
    })
}

/// `(wfd_id, environment)` için depoyu çözer. WFD'nin `$env`inde depo tanımlı değilse
/// deployment varsayılanı (`AppState.attachments`) döner.
pub async fn store_for_wfd(
    s: &AppState,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    environment_id: Option<Uuid>,
) -> Result<Arc<AttachmentStore>, AppError> {
    let owner = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let Some((project_id, wfd_name)) = owner else {
        return Ok(s.attachments.clone());
    };

    // Ortam verilmemişse tenant varsayılanı — `$env` çözümü bir ortam kimliği ister.
    let env_id = match environment_id {
        Some(id) => id,
        None => wf_wfe::repo::env::resolve_environment(&s.pool, orgtnt_id, None)
            .await
            .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?
            .id,
    };

    let run_env =
        wf_wfe::repo::env::load_run_env(&s.pool, project_id, &wfd_name, env_id, true)
            .await
            .map_err(|e| AppError(e.to_string(), StatusCode::UNPROCESSABLE_ENTITY))?;

    let Some(cfg) = config_from_env(&run_env, &s.cfg.attachment_storage.path) else {
        return Ok(s.attachments.clone());
    };

    let key = cache_key(&cfg);
    if let Some(hit) = cache().lock().expect("attachment store cache").get(&key) {
        return Ok(hit.clone());
    }
    let op = wf_wfd::build_operator(&cfg).map_err(|e| {
        AppError(
            format!("ek-belge deposu kurulamadı: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })?;
    let store = Arc::new(AttachmentStore::new(op));
    cache()
        .lock()
        .expect("attachment store cache")
        .insert(key, store.clone());
    Ok(store)
}

/// Var olan bir WFE'nin deposu — WFD'si ve ortamı satırdan okunur.
pub async fn store_for_wfe(s: &AppState, wfe_id: Uuid) -> Result<Arc<AttachmentStore>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
        "SELECT wfd_id, orgtnt_id, environment_id FROM wf.wfe WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    match row {
        Some((wfd_id, orgtnt_id, env_id)) => store_for_wfd(s, wfd_id, orgtnt_id, env_id).await,
        // WFE yoksa (silinmiş/başlamamış) varsayılan depo: çağıranlar zaten kendi
        // 404'lerini üretir, burada ikinci bir hata yolu açmıyoruz.
        None => Ok(s.attachments.clone()),
    }
}
