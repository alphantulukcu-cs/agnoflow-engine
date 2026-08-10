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
//! Hiçbiri tanımlı değilse RUNTIME'da deployment varsayılanına düşülür — belge toplamayan
//! akışlar etkilenmez. Ama **belge TOPLAYAN bir akış bu ayarlar olmadan YAYINLANAMAZ**
//! (`routes::wfd::assert_attachment_storage_env`): sessizce sunucu diskine yazmak, yayını
//! durdurmaktan pahalıdır. Editör bu yüzden anahtarları ortam tablosuna kendisi açar —
//! tasarımcının adları bilmesi gerekmez, yalnız değerleri girer.
//!
//! **Operator önbelleklenir.** S3 istemcisi kurmak her istekte yapılacak bir iş değildir;
//! anahtar, çözülmüş konfigürasyonun kendisidir (aynı config → aynı Operator). Konfigürasyon
//! değişince anahtar da değişir, eski girdi kullanılmaz kalır.
//!
//! ## Fallback'siz çözüm (ad-hoc not dosyası, 2026-08-10)
//!
//! Yukarıdaki `store_for_wfd`/`store_for_wfe` katalog attachment'ları için var: fallback
//! kasıtlıdır, çünkü sessiz düşüş publish kapısıyla (`assert_attachment_storage_env`)
//! önceden engellenmiştir — belge toplamayan akış bu kapıya hiç çarpmaz.
//!
//! Ad-hoc not dosyası (`docs/superpowers/specs/2026-08-10-wfe-not-ve-adhoc-belge-design.md`,
//! K4) o kapıdan geçmez: not ekleme WFD tasarımında öngörülemeyen, HER akışta ve HER adımda
//! açık bir yetenektir. Belge iliştirmeyen yüzlerce akışı "attachment storage tanımla ki
//! yayınlanabilesin" diye zorlamak yanlış olurdu (K4, reddedilen alternatifler). Bu yüzden
//! kapı publish zamanına değil, **runtime'a ve yalnız not-dosyası rotasına** taşınır:
//! `store_for_wfd_strict`/`store_for_wfe_strict` fallback'e hiç düşmez, `$env`'de gerekli
//! anahtarlar DEĞER olarak tanımlı değilse `422 code:"attachment_storage.missing_env"`
//! döner. Gerekçe aynı: müşterinin notuna eklediği belge, sessizce bizim sunucu diskine
//! yazılmamalı — katalokta bu güvence publish kapısıyla veriliyor, ad-hoc dosyada güvence
//! her çağrıda tazelenir çünkü publish anında hiçbir WFD bunu öngöremez.
//!
//! Ortak DB/`$env` çözümleme mantığı TEKRARLANMAZ: `store_for_wfd_impl`/`store_for_wfe_impl`
//! bir `strict: bool` alır, dört genel fonksiyon bunun üzerine ince kabuk olarak oturur.

use crate::{attachments::AttachmentStore, error::AppError, state::AppState};
use axum::http::StatusCode;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;
use wf_wfd::{StorageBackend, StorageConfig};
use wfe_core::types::wfd_v22::Wfd;
use wfe_core::v22::env::{EnvLookup, RunEnv};

/// Depo anahtarlarının SÖZLEŞMESİ — tek kaynak. Editörün ortam değişkenleri tablosu bu
/// adları otomatik açar (`utils/attachmentStorageEnv.ts`), yayın kapısı bu adları arar
/// (`routes::wfd::assert_attachment_storage_env`). Adlar burada değişirse üç yer birlikte
/// güncellenir; bir tanesi kalırsa "girilmiş ama okunmayan" bir ayar oluşur.
pub const KEY_BACKEND: &str = "ATTACHMENT_STORAGE_BACKEND";
pub const KEY_PATH: &str = "ATTACHMENT_STORAGE_PATH";
pub const KEY_S3_BUCKET: &str = "ATTACHMENT_STORAGE_S3_BUCKET";
pub const KEY_S3_REGION: &str = "ATTACHMENT_STORAGE_S3_REGION";
pub const KEY_S3_ENDPOINT: &str = "ATTACHMENT_STORAGE_S3_ENDPOINT";
pub const KEY_S3_ACCESS_KEY_ID: &str = "ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID";
pub const KEY_S3_SECRET_ACCESS_KEY: &str = "ATTACHMENT_STORAGE_S3_SECRET_ACCESS_KEY";

/// `backend` değerine göre DOLU olması gereken anahtarlar. Tanınmayan backend → `None`
/// (çağıran bunu "geçersiz seçim" hatasına çevirir; `config_from_env`in sessizce
/// varsayılana düşmesiyle aynı gerekçe — yanlış yazılmış bir backend değeri belgeleri
/// müşterinin bucket'ı yerine sunucu diskine yazdırır).
///
/// **`_S3_ENDPOINT` de listededir** (2026-08-10). Başta "AWS'de bölge yeter" diye
/// çıkarılmıştı; yanlış eksen: `wf_wfd::build_operator` endpoint'i ancak VERİLDİĞİNDE
/// uygular ve `disable_config_load()`/`disable_ec2_metadata()`'yı da yalnız o zaman çağırır
/// → boş endpoint sessizce AWS'e konuşur ve makinedeki ambient AWS credential'larını
/// kullanabilir. Garage/MinIO niyetiyle boş bırakılmış bir endpoint, tam olarak bu kapının
/// önlemeye çalıştığı "belgeler yanlış yere yazıldı" durumudur. AWS kullanan akış
/// endpoint'i açıkça yazar (`https://s3.<region>.amazonaws.com`).
pub fn required_env_keys(backend: &str) -> Option<&'static [&'static str]> {
    match backend.trim().to_ascii_lowercase().as_str() {
        "local" => Some(&[KEY_PATH]),
        "s3" => Some(&[
            KEY_S3_BUCKET,
            KEY_S3_REGION,
            KEY_S3_ENDPOINT,
            KEY_S3_ACCESS_KEY_ID,
            KEY_S3_SECRET_ACCESS_KEY,
        ]),
        _ => None,
    }
}

/// Bu WFD'de gerçekten DOSYA TOPLANIYOR mu? Katalogda grup olması yetmez (kullanılmayan
/// grup dosya üretmez); bir node'un referans verdiği ve İÇİNDE dosya slotu olan bir grup
/// gerekir. Yayın kapısı bunu sorar: depo ayarını yalnız belge yükleyen akışlarda zorunlu
/// tutmak, belgesiz akışları etkilemez.
pub fn collects_attachments(wfd: &Wfd) -> bool {
    wfd.nodes.values().any(|node| {
        node.attachments.iter().any(|r| {
            wfd.attachments
                .get(r.group())
                .is_some_and(|g| !g.items.is_empty())
        })
    })
}

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

/// Ad-hoc not dosyası rotasının fallback'siz kapısı — `$env`'de backend değeri ya da
/// backend'in gerektirdiği anahtarlardan biri DEĞER olarak tanımlı değilse `422
/// code:"attachment_storage.missing_env"`. Sıra: önce `KEY_BACKEND` (yoksa/tanınmıyorsa
/// diğer anahtarlar hiç aranmaz — hangi kümenin gerekli olduğu backend'den türer), sonra
/// backend'in kümesindeki eksik anahtarlar, TÜMÜ birden mesaja yazılır.
fn missing_required_env_keys(env: &RunEnv) -> Vec<&'static str> {
    let Some(backend) = env_str(env, KEY_BACKEND) else {
        return vec![KEY_BACKEND];
    };
    let Some(keys) = required_env_keys(&backend) else {
        return vec![KEY_BACKEND];
    };
    keys.iter()
        .copied()
        .filter(|k| env_str(env, k).is_none())
        .collect()
}

/// `store_for_*_strict` için tek üretici: 422 + Türkçe mesaj + makine-okunur kod.
fn missing_env_error(missing: &[&str]) -> AppError {
    AppError {
        message: format!(
            "attachment_storage.missing_env — nota belge iliştirilemedi: $env'de eksik anahtar(lar): {}",
            missing.join(", ")
        ),
        status: StatusCode::UNPROCESSABLE_ENTITY,
        code: Some("attachment_storage.missing_env"),
    }
}

/// `store_for_wfd`/`store_for_wfd_strict`in paylaştığı çözümleme. `strict = false`
/// (katalog davranışı, DEĞİŞMEDİ): `$env`de depo tanımsızsa deployment varsayılanına
/// sessizce düşer. `strict = true` (ad-hoc not dosyası, K4): aynı düşüşler yerine
/// `missing_env_error` döner — hiçbir dalda `s.attachments` varsayılanına geçilmez.
async fn store_for_wfd_impl(
    s: &AppState,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    environment_id: Option<Uuid>,
    strict: bool,
) -> Result<Arc<AttachmentStore>, AppError> {
    let owner = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT project_id, name FROM wf.wfd_meta WHERE wfd_id = $1",
    )
    .bind(wfd_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    let Some((project_id, wfd_name)) = owner else {
        // Projesi olmayan (eski/serbest) WFD'nin `$env` sahipliği yoktur. Katalog
        // davranışında (strict=false) varsayılana düşülür (mevcut davranış). Strict'te
        // düşülecek bir "belirli ayar" yok — yine de fallback'e izin VERİLMEZ: backend
        // hiç çözülemediği için eksik sayılır.
        if strict {
            return Err(missing_env_error(&[KEY_BACKEND]));
        }
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

    if strict {
        let missing = missing_required_env_keys(&run_env);
        if !missing.is_empty() {
            return Err(missing_env_error(&missing));
        }
    }

    let Some(cfg) = config_from_env(&run_env, &s.cfg.attachment_storage.path) else {
        // strict=true buraya düşmez (yukarıdaki kontrol zaten backend'i doğruladı);
        // savunma amaçlı aynı hatayı üretir, sessiz varsayılana asla geçmez.
        if strict {
            return Err(missing_env_error(&[KEY_BACKEND]));
        }
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

/// `(wfd_id, environment)` için depoyu çözer. WFD'nin `$env`inde depo tanımlı değilse
/// deployment varsayılanı (`AppState.attachments`) döner. Katalog attachment rotalarının
/// kullandığı davranış — DEĞİŞMEDİ.
pub async fn store_for_wfd(
    s: &AppState,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    environment_id: Option<Uuid>,
) -> Result<Arc<AttachmentStore>, AppError> {
    store_for_wfd_impl(s, wfd_id, orgtnt_id, environment_id, false).await
}

/// `store_for_wfd`in fallback'siz varyantı — ad-hoc not dosyası rotası içindir (K4).
/// `$env`de gerekli anahtarlar DEĞER olarak tanımlı değilse deployment varsayılanına
/// asla düşmez, `422 code:"attachment_storage.missing_env"` döner.
pub async fn store_for_wfd_strict(
    s: &AppState,
    wfd_id: Uuid,
    orgtnt_id: Uuid,
    environment_id: Option<Uuid>,
) -> Result<Arc<AttachmentStore>, AppError> {
    store_for_wfd_impl(s, wfd_id, orgtnt_id, environment_id, true).await
}

/// `store_for_wfe`/`store_for_wfe_strict`in paylaştığı çözümleme — bkz. `store_for_wfd_impl`.
async fn store_for_wfe_impl(
    s: &AppState,
    wfe_id: Uuid,
    strict: bool,
) -> Result<Arc<AttachmentStore>, AppError> {
    let row = sqlx::query_as::<_, (Uuid, Uuid, Option<Uuid>)>(
        "SELECT wfd_id, orgtnt_id, environment_id FROM wf.wfe WHERE wfe_id = $1",
    )
    .bind(wfe_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(|e| AppError(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR))?;
    match row {
        Some((wfd_id, orgtnt_id, env_id)) => {
            store_for_wfd_impl(s, wfd_id, orgtnt_id, env_id, strict).await
        }
        // WFE yoksa (silinmiş/başlamamış): katalog davranışında (strict=false) varsayılan
        // depo döner — çağıranlar zaten kendi 404'lerini üretir, burada ikinci bir hata
        // yolu açmıyoruz. Strict'te aynı gerekçeyle fallback'e izin verilmez: WFE'si
        // bulunamayan bir isteğe sessizce deployment deposu vermek K4'ün önlediği şeydir.
        None => {
            if strict {
                Err(missing_env_error(&[KEY_BACKEND]))
            } else {
                Ok(s.attachments.clone())
            }
        }
    }
}

/// Var olan bir WFE'nin deposu — WFD'si ve ortamı satırdan okunur. Katalog attachment
/// rotalarının kullandığı davranış — DEĞİŞMEDİ.
pub async fn store_for_wfe(s: &AppState, wfe_id: Uuid) -> Result<Arc<AttachmentStore>, AppError> {
    store_for_wfe_impl(s, wfe_id, false).await
}

/// `store_for_wfe`in fallback'siz varyantı — ad-hoc not dosyası rotası içindir (K4).
/// Bkz. `store_for_wfd_strict` doc yorumu.
pub async fn store_for_wfe_strict(
    s: &AppState,
    wfe_id: Uuid,
) -> Result<Arc<AttachmentStore>, AppError> {
    store_for_wfe_impl(s, wfe_id, true).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Tek node + tek grup taşıyan en küçük belge. `items` ve node referansı testten
    /// parametrelenir — kapının sorduğu tam olarak bu ikisidir.
    fn wfd_with(items: serde_json::Value, node_refs: serde_json::Value) -> Wfd {
        Wfd::from_value(json!({
            "wfd_version": "2.2",
            "id": "t", "name": "t", "version": "1",
            "context": { "type": "object", "properties": {} },
            "nodes": {
                "memur": {
                    "c_a": { "c_orgu": "self", "c_r": ["memur"] },
                    "attachments": node_refs
                }
            },
            "start": [], "actions": {}, "transitions": [], "terminals": [],
            "attachments": { "evraklar": { "items": items } }
        }))
        .expect("minimal wfd")
    }

    #[test]
    fn collects_when_node_refers_to_nonempty_group() {
        let wfd = wfd_with(json!([{ "id": "kimlik" }]), json!(["evraklar"]));
        assert!(collects_attachments(&wfd));
        // Kapsamlı referans da toplar — `actions` yalnız KAPIYI daraltır, dosya yine yüklenir.
        let scoped = wfd_with(
            json!([{ "id": "kimlik" }]),
            json!([{ "group": "evraklar", "actions": [] }]),
        );
        assert!(collects_attachments(&scoped));
    }

    #[test]
    fn does_not_collect_without_reference_or_items() {
        // Katalogda grup var ama hiçbir node toplamıyor → dosya üretilmez.
        assert!(!collects_attachments(&wfd_with(
            json!([{ "id": "kimlik" }]),
            json!([])
        )));
        // Referans var ama grubun dosya slotu yok → yüklenecek bir şey yok.
        assert!(!collects_attachments(&wfd_with(json!([]), json!(["evraklar"]))));
    }

    #[test]
    fn required_keys_follow_backend() {
        assert_eq!(required_env_keys("local"), Some(&[KEY_PATH][..]));
        assert_eq!(
            required_env_keys(" S3 "),
            Some(
                &[
                    KEY_S3_BUCKET,
                    KEY_S3_REGION,
                    KEY_S3_ENDPOINT,
                    KEY_S3_ACCESS_KEY_ID,
                    KEY_S3_SECRET_ACCESS_KEY
                ][..]
            )
        );
        // ENDPOINT ZORUNLU: boş bırakılırsa `build_operator` AWS'e konuşur ve ambient
        // credential zinciri açık kalır (bkz. `required_env_keys` notu).
        assert!(required_env_keys("s3").unwrap().contains(&KEY_S3_ENDPOINT));
        // Tanınmayan backend sessizce local'a düşmez — çağıran hata verir.
        assert_eq!(required_env_keys("S£"), None);
        assert_eq!(required_env_keys(""), None);
    }
}
