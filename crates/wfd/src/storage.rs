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

/// `<PREFIX>_BACKEND` / `_PATH` / `_S3_*` adlarını verilen arayıcıdan okuyup depo
/// konfigürasyonu üretir.
///
/// Ad kümesi SÖZLEŞMEDİR (`ATTACHMENT_STORAGE_*` adları WFD ayarları ekranında
/// tasarımcı tarafından bu adlarla girilir) ve **iki ayrı kaynaktan** sorulur:
/// deployment ortamı (`std::env`) ve WFD'nin `$env` satırları (`wf.wfd_env_var`).
/// İkinci bir kopya çıkarsa biri güncellenip diğeri unutulur — bu depoda kopya
/// disiplininin üç kez kırıldığı kayıtlı (bkz. `wfd-v2.3/00-BAGLAM.md` §4).
///
/// `None` iki hâlde döner: `_BACKEND` yok, ya da değeri TANINMIYOR. Tanınmayan
/// değerin sessizce `local`a düşmemesi bilinçlidir (`server::attachment_store`):
/// yanlış yazılmış bir backend, belgeleri müşterinin bucket'ı yerine sunucu diskine
/// yazdırır ve bu, fark edilmesi en zor hata sınıfıdır. Çağıran `None`'ı ya hata
/// yapar ya da kendi varsayılanına düşer — karar onun.
///
/// `lookup` boş dizeyi "tanımlı değil" saymalıdır (`$env` tarafı öyle sayıyor);
/// `_PATH` yoksa `fallback_path` kullanılır.
pub fn storage_config_from_lookup(
    prefix: &str,
    lookup: impl Fn(&str) -> Option<String>,
    fallback_path: &str,
) -> Option<StorageConfig> {
    let key = |suffix: &str| format!("{prefix}_{suffix}");
    let backend = match lookup(&key("BACKEND"))?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "s3" => StorageBackend::S3,
        "local" => StorageBackend::Local,
        _ => return None,
    };
    Some(StorageConfig {
        backend,
        path: lookup(&key("PATH")).unwrap_or_else(|| fallback_path.to_string()),
        s3_bucket: lookup(&key("S3_BUCKET")),
        s3_region: lookup(&key("S3_REGION")),
        s3_endpoint: lookup(&key("S3_ENDPOINT")),
        s3_access_key_id: lookup(&key("S3_ACCESS_KEY_ID")),
        s3_secret_access_key: lookup(&key("S3_SECRET_ACCESS_KEY")),
    })
}

/// Ek-belge deposunun `$env`/env ad öneki. WFD ayarları ekranında tasarımcı bu
/// önekle girer; sözleşmedir.
pub const ATTACHMENT_ENV_PREFIX: &str = "ATTACHMENT_STORAGE";

/// Local backend'de ek-belge kökü — engine cwd'sine göre. WFD JSON deposundan
/// (`STORAGE_PATH`) AYRI konum: dış UI'ın yüklediği dosyalar burada tutulur.
pub const DEFAULT_ATTACHMENT_PATH: &str = "../work-pool-portal/storage";

/// Ek-belge deposunun DEPLOYMENT varsayılanı (`ATTACHMENT_STORAGE_*` env'i).
///
/// `_BACKEND` yokluğu/tanınmaması burada hata DEĞİL, yerel köke düşülür: belge
/// toplamayan yüzlerce akış bu ayarı hiç girmez. WFD başına `$env` override'ı
/// sunucu tarafındadır (`server::attachment_store`) ve o yol yazmada fallback'e
/// DÜŞMEZ; bu fonksiyon yalnız tabanı verir.
pub fn attachment_storage_from_env() -> StorageConfig {
    storage_config_from_lookup(
        ATTACHMENT_ENV_PREFIX,
        |key| std::env::var(key).ok().filter(|v| !v.trim().is_empty()),
        DEFAULT_ATTACHMENT_PATH,
    )
    .unwrap_or_else(|| StorageConfig {
        backend: StorageBackend::Local,
        path: std::env::var("ATTACHMENT_STORAGE_PATH")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ATTACHMENT_PATH.into()),
        s3_bucket: None,
        s3_region: None,
        s3_endpoint: None,
        s3_access_key_id: None,
        s3_secret_access_key: None,
    })
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

/// Senaryo sidecar'ının storage anahtarı — kaydedilmiş simülasyon koşuları
/// (`{version}.scenarios.json`). Layout ile aynı gerekçe: doküman
/// `additionalProperties:false` ve `(wfd_id, version)` immutable olduğundan
/// senaryolar gövdeye giremez, dokümanın YANINDA durur.
///
/// Layout'un aksine legacy (tenant-öncesi) karşılığı YOKTUR — bu anahtar tenant
/// prefix'i yerleştikten sonra doğdu.
pub fn scenarios_key(orgtnt_id: uuid::Uuid, wfd_id: uuid::Uuid, version: i32) -> String {
    format!("{orgtnt_id}/wfd/{wfd_id}/{version}.scenarios.json")
}

/// Tenant-öncesi (eski, tek-tenant) layout anahtarı. Layout anahtarı DB'de
/// SAKLANMADIĞINDAN türetilir; tenant prefix'ine geçişte eski bloblar bu
/// anahtarda kalır. `fetch_layout` yeni anahtar bulunamazsa buna düşer.
pub fn legacy_layout_key(wfd_id: uuid::Uuid, version: i32) -> String {
    format!("wfd/{wfd_id}/{version}.layout.json")
}

/// Tenant'ın marka varlığı (logo/favicon) anahtarı — WFD JSON ile AYNI tenant kökü
/// altında, `logo/` dizininde: `{orgtnt_id}/logo/{slot}.{ext}`.
///
/// Uzantı mime'dan türetilir; tam anahtar DB'de saklanır, böylece uzantı değiştiren
/// yeniden yüklemede eski blob silinebilir.
pub fn tenant_asset_key(orgtnt_id: uuid::Uuid, slot: &str, ext: &str) -> String {
    format!("{orgtnt_id}/logo/{slot}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenarios_key_sits_next_to_the_document() {
        let t = uuid::Uuid::nil();
        let w = uuid::Uuid::nil();
        assert_eq!(
            scenarios_key(t, w, 3),
            "00000000-0000-0000-0000-000000000000/wfd/00000000-0000-0000-0000-000000000000/3.scenarios.json"
        );
        // Layout ile aynı dizinde, aynı versiyon önekinde.
        let layout = layout_key(t, w, 3);
        let scenarios = scenarios_key(t, w, 3);
        assert_eq!(
            layout.rsplit_once('/').unwrap().0,
            scenarios.rsplit_once('/').unwrap().0
        );
    }

    #[test]
    fn tenant_asset_lives_under_tenant_logo_dir() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            tenant_asset_key(id, "logo", "png"),
            "00000000-0000-0000-0000-000000000000/logo/logo.png"
        );
        assert_eq!(
            tenant_asset_key(id, "favicon", "ico"),
            "00000000-0000-0000-0000-000000000000/logo/favicon.ico"
        );
        // WFD JSON ile aynı tenant kökünü paylaşır.
        let wfd = s3_key(id, uuid::Uuid::nil(), 1);
        let asset = tenant_asset_key(id, "logo", "svg");
        assert_eq!(
            wfd.split('/').next().unwrap(),
            asset.split('/').next().unwrap()
        );
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;
    use std::collections::HashMap;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn cfg(pairs: &[(&str, &str)]) -> Option<StorageConfig> {
        let m = map(pairs);
        storage_config_from_lookup(ATTACHMENT_ENV_PREFIX, |k| m.get(k).cloned(), "/fallback")
    }

    #[test]
    fn backend_yoksa_none() {
        assert!(cfg(&[("ATTACHMENT_STORAGE_PATH", "/x")]).is_none());
    }

    /// Tanınmayan backend SESSİZCE local'a düşmez — yanlış yazılmış bir değer,
    /// belgeleri müşterinin bucket'ı yerine sunucu diskine yazdırırdı.
    #[test]
    fn taninmayan_backend_none() {
        assert!(cfg(&[("ATTACHMENT_STORAGE_BACKEND", "S£")]).is_none());
    }

    #[test]
    fn backend_kirpilir_ve_kucuk_harfe_cevrilir() {
        let c = cfg(&[("ATTACHMENT_STORAGE_BACKEND", " S3 ")]).expect("s3");
        assert!(matches!(c.backend, StorageBackend::S3));
    }

    #[test]
    fn path_yoksa_fallback() {
        let c = cfg(&[("ATTACHMENT_STORAGE_BACKEND", "local")]).expect("local");
        assert_eq!(c.path, "/fallback");
    }

    #[test]
    fn s3_alanlari_onekle_okunur() {
        let c = cfg(&[
            ("ATTACHMENT_STORAGE_BACKEND", "s3"),
            ("ATTACHMENT_STORAGE_S3_BUCKET", "b"),
            ("ATTACHMENT_STORAGE_S3_REGION", "garage"),
            ("ATTACHMENT_STORAGE_S3_ENDPOINT", "http://x:3900"),
            ("ATTACHMENT_STORAGE_S3_ACCESS_KEY_ID", "id"),
            ("ATTACHMENT_STORAGE_S3_SECRET_ACCESS_KEY", "sec"),
        ])
        .expect("s3");
        assert_eq!(c.s3_bucket.as_deref(), Some("b"));
        assert_eq!(c.s3_region.as_deref(), Some("garage"));
        assert_eq!(c.s3_endpoint.as_deref(), Some("http://x:3900"));
        assert_eq!(c.s3_access_key_id.as_deref(), Some("id"));
        assert_eq!(c.s3_secret_access_key.as_deref(), Some("sec"));
    }

    /// Önek gerçekten uygulanıyor: WFD JSON deposunun adları ek-belge deposuna
    /// SIZMAZ (iki depo AYRI konumdur).
    #[test]
    fn onek_yanlissa_okumaz() {
        let m = map(&[("STORAGE_BACKEND", "s3")]);
        assert!(
            storage_config_from_lookup(ATTACHMENT_ENV_PREFIX, |k| m.get(k).cloned(), "/f")
                .is_none()
        );
    }
}
