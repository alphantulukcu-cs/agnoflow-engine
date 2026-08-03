//! Tenant marka varlıkları (logo/favicon) — doğrulama + depolama.
//!
//! Bayt'lar WFD JSON ile AYNI tenant-prefixli object storage'da, `logo/` dizininde
//! durur (`{orgtnt_id}/logo/{slot}.{ext}`); DB yalnız anahtar + mime + zaman damgası
//! taşır. Böylece S3/FS düzeyinde prefix bazlı tenant izolasyonu (IAM policy,
//! listeleme, silme) marka varlıklarını da kapsar.
//!
//! Bu modül HTTP route'larından bağımsızdır: hem admin ağacı
//! (`routes/org_branding.rs`, X-Admin-Key) hem portal ağacı
//! (`routes/portal/branding.rs`, JWT) aynı fonksiyonları çağırır.

use crate::error::AppError;
use axum::body::Bytes;
use axum::http::{header, HeaderMap, StatusCode};
use chrono::{DateTime, Utc};
use opendal::Operator;
use serde::Serialize;
use sqlx::PgPool;
use utoipa::ToSchema;
use uuid::Uuid;
use wf_org::models::{BrandAsset, Orgtnt};
use wf_org::repo;

/// Logo için kabul edilen `(mime, uzantı)` — uzantı storage anahtarına girer.
const LOGO_TYPES: &[(&str, &str)] = &[
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/svg+xml", "svg"),
];

/// Favicon logo tiplerinin tamamını + ICO'yu kabul eder.
const FAVICON_EXTRA_TYPES: &[(&str, &str)] = &[
    ("image/x-icon", "ico"),
    ("image/vnd.microsoft.icon", "ico"),
];

pub const LOGO_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const FAVICON_MAX_BYTES: usize = 512 * 1024;

fn types_for(slot: BrandAsset) -> Vec<(&'static str, &'static str)> {
    let mut v = LOGO_TYPES.to_vec();
    if slot == BrandAsset::Favicon {
        v.extend_from_slice(FAVICON_EXTRA_TYPES);
    }
    v
}

pub fn max_bytes(slot: BrandAsset) -> usize {
    match slot {
        BrandAsset::Logo => LOGO_MAX_BYTES,
        BrandAsset::Favicon => FAVICON_MAX_BYTES,
    }
}

/// Yol parçasını slot'a çevirir; bilinmeyen slot 404'tür (route yolu gibi davranır).
pub fn parse_slot(slot: &str) -> Result<BrandAsset, AppError> {
    BrandAsset::parse(slot).ok_or_else(|| {
        AppError(
            format!("bilinmeyen varlık slotu: {slot} (logo|favicon)"),
            StatusCode::NOT_FOUND,
        )
    })
}

/// Yükleme doğrulaması → storage anahtarı uzantısı.
pub fn validate(slot: BrandAsset, mime: &str, len: usize) -> Result<&'static str, AppError> {
    if len == 0 {
        return Err(AppError(
            "boş dosya yüklenemez".into(),
            StatusCode::BAD_REQUEST,
        ));
    }
    let types = types_for(slot);
    let ext = types.iter().find(|(m, _)| *m == mime).map(|(_, e)| *e);
    let Some(ext) = ext else {
        let accepted: Vec<&str> = types.iter().map(|(m, _)| *m).collect();
        return Err(AppError(
            format!(
                "izin verilmeyen içerik tipi: {} (kabul edilenler: {})",
                if mime.is_empty() {
                    "(içerik tipi yok)"
                } else {
                    mime
                },
                accepted.join(", ")
            ),
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
        ));
    };
    let max = max_bytes(slot);
    if len > max {
        return Err(AppError(
            format!(
                "dosya {:.0} KB sınırını aşıyor",
                max as f64 / 1024.0
            ),
            StatusCode::PAYLOAD_TOO_LARGE,
        ));
    }
    Ok(ext)
}

fn storage_err(op: &str, e: opendal::Error) -> AppError {
    AppError(
        format!("marka varlığı {op} başarısız: {e}"),
        StatusCode::INTERNAL_SERVER_ERROR,
    )
}

/// Varlığı yazar ve DB referansını günceller.
///
/// Sıra: blob YAZ → DB güncelle → eski blob'u (uzantı değiştiyse) sil. DB yazımı
/// başarısız olursa yeni blob yetim kalır ama referans eski varlığı gösterdiğinden
/// görünen durum tutarlıdır; bir sonraki başarılı yükleme aynı anahtarı ezer.
pub async fn store(
    op: &Operator,
    pool: &PgPool,
    tenant: &Orgtnt,
    slot: BrandAsset,
    mime: &str,
    bytes: Bytes,
) -> Result<Orgtnt, AppError> {
    let ext = validate(slot, mime, bytes.len())?;
    let key = wf_wfd::storage::tenant_asset_key(tenant.orgtnt_id, slot.as_str(), ext);

    op.write(&key, bytes.to_vec())
        .await
        .map_err(|e| storage_err("yükleme", e))?;

    let updated = repo::orgtnt::set_asset(pool, tenant.orgtnt_id, slot, Some((&key, mime))).await?;

    // Uzantı değiştiyse eski blob artık referanssız — best-effort temizle.
    if let Some((old_key, _)) = tenant.asset(slot) {
        if old_key != key {
            let _ = op.delete(old_key).await;
        }
    }
    Ok(updated)
}

/// Varlığı okur; yanıt başlıkları da burada kurulur.
///
/// Güvenlik: SVG logo üst düzey sekmede açılırsa script çalıştırabilir — bu yüzden
/// `nosniff` + katı CSP ile ve `inline` disposition ile servis edilir.
pub async fn read(
    op: &Operator,
    tenant: &Orgtnt,
    slot: BrandAsset,
) -> Result<(HeaderMap, Bytes), AppError> {
    let Some((key, mime)) = tenant.asset(slot) else {
        return Err(AppError(
            format!("{} yüklenmemiş", slot.as_str()),
            StatusCode::NOT_FOUND,
        ));
    };
    let bytes = op
        .read(key)
        .await
        .map_err(|_| AppError("marka varlığı bulunamadı".into(), StatusCode::NOT_FOUND))?;

    let mut h = HeaderMap::new();
    h.insert(
        header::CONTENT_TYPE,
        mime.parse()
            .unwrap_or_else(|_| "application/octet-stream".parse().unwrap()),
    );
    // Yetkiye bağlı içerik: paylaşımlı cache'lerde tutulmasın, ETag ile revalidate edilsin.
    h.insert(header::CACHE_CONTROL, "private, no-cache".parse().unwrap());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        "default-src 'none'; style-src 'unsafe-inline'; sandbox"
            .parse()
            .unwrap(),
    );
    if let Some(name) = key.rsplit('/').next() {
        if let Ok(v) = format!("inline; filename=\"{name}\"").parse() {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok((h, Bytes::from(bytes.to_vec())))
}

/// Varlığı siler: DB referansı temizlenir, blob best-effort silinir.
pub async fn remove(
    op: &Operator,
    pool: &PgPool,
    tenant: &Orgtnt,
    slot: BrandAsset,
) -> Result<Orgtnt, AppError> {
    let key = tenant.asset(slot).map(|(k, _)| k.to_string());
    let updated = repo::orgtnt::set_asset(pool, tenant.orgtnt_id, slot, None).await?;
    if let Some(key) = key {
        let _ = op.delete(&key).await;
    }
    Ok(updated)
}

/// Portal/istemci için marka özeti — bayt taşımaz, yalnız varlık DURUMUNU ve
/// görsel kimliği taşır. Logo'nun kendisi ayrı endpoint'ten çekilir.
#[derive(Debug, Serialize, ToSchema)]
pub struct BrandingSummary {
    pub orgtnt_id: Uuid,
    pub name: String,
    pub code: String,
    pub display_name: Option<String>,
    pub brand_color: Option<String>,
    pub locale: String,
    pub timezone: String,
    pub currency: String,
    pub has_logo: bool,
    pub logo_mime: Option<String>,
    pub logo_updated_at: Option<DateTime<Utc>>,
    pub has_favicon: bool,
    pub favicon_mime: Option<String>,
    pub favicon_updated_at: Option<DateTime<Utc>>,
}

impl From<&Orgtnt> for BrandingSummary {
    fn from(t: &Orgtnt) -> Self {
        Self {
            orgtnt_id: t.orgtnt_id,
            name: t.name.clone(),
            code: t.code.clone(),
            display_name: t.display_name.clone(),
            brand_color: t.brand_color.clone(),
            locale: t.locale.clone(),
            timezone: t.timezone.clone(),
            currency: t.currency.clone(),
            has_logo: t.asset(BrandAsset::Logo).is_some(),
            logo_mime: t.logo_mime.clone(),
            logo_updated_at: t.logo_updated_at,
            has_favicon: t.asset(BrandAsset::Favicon).is_some(),
            favicon_mime: t.favicon_mime.clone(),
            favicon_updated_at: t.favicon_updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_allowlist_maps_to_extension() {
        assert_eq!(validate(BrandAsset::Logo, "image/png", 10).unwrap(), "png");
        assert_eq!(validate(BrandAsset::Logo, "image/svg+xml", 10).unwrap(), "svg");
        // ICO yalnız favicon slotunda kabul edilir.
        assert_eq!(
            validate(BrandAsset::Favicon, "image/x-icon", 10).unwrap(),
            "ico"
        );
        let err = validate(BrandAsset::Logo, "image/x-icon", 10).unwrap_err();
        assert_eq!(err.status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[test]
    fn rejects_empty_unknown_and_oversized() {
        assert_eq!(
            validate(BrandAsset::Logo, "image/png", 0).unwrap_err().status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            validate(BrandAsset::Logo, "application/pdf", 10)
                .unwrap_err()
                .status,
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        assert_eq!(
            validate(BrandAsset::Logo, "image/png", LOGO_MAX_BYTES + 1)
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        // Favicon sınırı daha dar: logo için geçerli boyut burada reddedilir.
        assert_eq!(
            validate(BrandAsset::Favicon, "image/png", FAVICON_MAX_BYTES + 1)
                .unwrap_err()
                .status,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn unknown_slot_is_not_found() {
        assert_eq!(
            parse_slot("banner").unwrap_err().status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(parse_slot("logo").unwrap(), BrandAsset::Logo);
    }
}
