use crate::{
    error::OrgError,
    models::{BrandAsset, Orgtnt},
};
use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

/// Tüm SELECT/RETURNING'lerde aynı kolon sırası — `Orgtnt` alanlarıyla birebir.
const COLS: &str = "orgtnt_id, name, code, display_name, brand_color, legal_name, tax_no, \
     tax_office, contact_email, contact_phone, website, address, city, country, timezone, \
     locale, currency, external_id, logo_key, logo_mime, logo_updated_at, favicon_key, \
     favicon_mime, favicon_updated_at, settings, is_active, created_at, updated_at";

/// Tenant güncelleme yaması.
///
/// Semantik (API sözleşmesi): alan **gönderilmediyse** (`None`) değişmez; **boş
/// string** gönderilirse temizlenir (NULL). Zorunlu alanlarda (`name`, `code`,
/// yerelleştirme) boş string hatadır — bkz. [`take_required`].
#[derive(Debug, Default, Clone)]
pub struct OrgtntPatch {
    pub name: Option<String>,
    pub code: Option<String>,
    pub display_name: Option<String>,
    pub brand_color: Option<String>,
    pub legal_name: Option<String>,
    pub tax_no: Option<String>,
    pub tax_office: Option<String>,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub website: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub country: Option<String>,
    pub timezone: Option<String>,
    pub locale: Option<String>,
    pub currency: Option<String>,
    pub external_id: Option<String>,
    pub settings: Option<serde_json::Value>,
    pub is_active: Option<bool>,
}

/// Zorunlu metin: gönderilmediyse mevcut değer korunur, boş gönderilirse hata.
fn take_required(new: Option<String>, cur: String, field: &str) -> Result<String, OrgError> {
    match new {
        None => Ok(cur),
        Some(v) => {
            let t = v.trim();
            if t.is_empty() {
                Err(OrgError::BadRequest(format!("{field} boş olamaz")))
            } else {
                Ok(t.to_string())
            }
        }
    }
}

/// Opsiyonel metin: gönderilmediyse korunur, boş gönderilirse NULL'a çekilir.
fn take_optional(new: Option<String>, cur: Option<String>) -> Option<String> {
    match new {
        None => cur,
        Some(v) => {
            let t = v.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
    }
}

pub async fn list(pool: &PgPool, limit: i64, offset: i64) -> Result<Vec<Orgtnt>, OrgError> {
    sqlx::query_as::<_, Orgtnt>(&format!(
        "SELECT {COLS} FROM org.orgtnt ORDER BY name LIMIT $1 OFFSET $2"
    ))
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await
    .map_err(OrgError::Database)
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Orgtnt, OrgError> {
    sqlx::query_as::<_, Orgtnt>(&format!("SELECT {COLS} FROM org.orgtnt WHERE orgtnt_id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}

pub async fn create(pool: &PgPool, name: String, code: String) -> Result<Orgtnt, OrgError> {
    let id = Uuid::new_v4();
    let now = Utc::now();

    sqlx::query_as::<_, Orgtnt>(&format!(
        "INSERT INTO org.orgtnt (orgtnt_id, name, code, is_active, created_at, updated_at)
         VALUES ($1, $2, $3, true, $4, $5)
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(&name)
    .bind(&code)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await
    .map_err(OrgError::Database)
}

/// Yamayı mevcut satırın üzerine uygular. Okuma+yazma TEK transaction'da,
/// `FOR UPDATE` kilidiyle yürür — eşzamanlı iki PATCH birbirinin alanını ezmez.
///
/// Ülke/para birimi büyük, marka rengi küçük harfe normalize edilir ki DB CHECK'leri
/// kullanıcının yazımına takılmasın.
pub async fn patch(pool: &PgPool, id: Uuid, p: OrgtntPatch) -> Result<Orgtnt, OrgError> {
    let mut tx = pool.begin().await?;

    let cur = sqlx::query_as::<_, Orgtnt>(&format!(
        "SELECT {COLS} FROM org.orgtnt WHERE orgtnt_id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))?;

    let name = take_required(p.name, cur.name, "name")?;
    let code = take_required(p.code, cur.code, "code")?;
    let timezone = take_required(p.timezone, cur.timezone, "timezone")?;
    let locale = take_required(p.locale, cur.locale, "locale")?;
    let currency = take_required(p.currency, cur.currency, "currency")?.to_uppercase();
    let settings = match p.settings {
        None => cur.settings,
        Some(v) if v.is_object() => v,
        Some(_) => {
            return Err(OrgError::BadRequest(
                "settings bir JSON object olmalı".into(),
            ))
        }
    };

    let updated = sqlx::query_as::<_, Orgtnt>(&format!(
        "UPDATE org.orgtnt SET
            name = $2, code = $3, display_name = $4, brand_color = $5, legal_name = $6,
            tax_no = $7, tax_office = $8, contact_email = $9, contact_phone = $10,
            website = $11, address = $12, city = $13, country = $14, timezone = $15,
            locale = $16, currency = $17, external_id = $18, settings = $19,
            is_active = $20, updated_at = $21
         WHERE orgtnt_id = $1
         RETURNING {COLS}"
    ))
    .bind(id)
    .bind(&name)
    .bind(&code)
    .bind(take_optional(p.display_name, cur.display_name))
    .bind(take_optional(p.brand_color, cur.brand_color).map(|s| s.to_lowercase()))
    .bind(take_optional(p.legal_name, cur.legal_name))
    .bind(take_optional(p.tax_no, cur.tax_no))
    .bind(take_optional(p.tax_office, cur.tax_office))
    .bind(take_optional(p.contact_email, cur.contact_email))
    .bind(take_optional(p.contact_phone, cur.contact_phone))
    .bind(take_optional(p.website, cur.website))
    .bind(take_optional(p.address, cur.address))
    .bind(take_optional(p.city, cur.city))
    .bind(take_optional(p.country, cur.country).map(|s| s.to_uppercase()))
    .bind(&timezone)
    .bind(&locale)
    .bind(&currency)
    .bind(take_optional(p.external_id, cur.external_id))
    .bind(&settings)
    .bind(p.is_active.unwrap_or(cur.is_active))
    .bind(Utc::now())
    .fetch_one(&mut *tx)
    .await
    .map_err(OrgError::Database)?;

    tx.commit().await?;
    Ok(updated)
}

/// Marka varlığı referansını yazar (`Some`) veya temizler (`None`). Bayt'lar
/// storage'da durur; burada yalnız anahtar + mime + zaman damgası tutulur.
pub async fn set_asset(
    pool: &PgPool,
    id: Uuid,
    slot: BrandAsset,
    asset: Option<(&str, &str)>,
) -> Result<Orgtnt, OrgError> {
    let (key, mime) = match asset {
        Some((k, m)) => (Some(k), Some(m)),
        None => (None, None),
    };
    let asset_at = asset.map(|_| Utc::now());
    let sql = match slot {
        BrandAsset::Logo => format!(
            "UPDATE org.orgtnt SET logo_key = $2, logo_mime = $3, logo_updated_at = $4,
                updated_at = $5 WHERE orgtnt_id = $1 RETURNING {COLS}"
        ),
        BrandAsset::Favicon => format!(
            "UPDATE org.orgtnt SET favicon_key = $2, favicon_mime = $3, favicon_updated_at = $4,
                updated_at = $5 WHERE orgtnt_id = $1 RETURNING {COLS}"
        ),
    };
    sqlx::query_as::<_, Orgtnt>(&sql)
        .bind(id)
        .bind(key)
        .bind(mime)
        .bind(asset_at)
        .bind(Utc::now())
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| OrgError::NotFound(format!("orgtnt {id}")))
}

pub async fn delete(pool: &PgPool, id: Uuid) -> Result<(), OrgError> {
    let result = sqlx::query("DELETE FROM org.orgtnt WHERE orgtnt_id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(OrgError::Database)?;

    if result.rows_affected() == 0 {
        return Err(OrgError::NotFound(format!("orgtnt {id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_keeps_current_when_absent_and_rejects_blank() {
        assert_eq!(take_required(None, "Acme".into(), "name").unwrap(), "Acme");
        assert_eq!(
            take_required(Some("  Yeni  ".into()), "Acme".into(), "name").unwrap(),
            "Yeni"
        );
        assert!(take_required(Some("   ".into()), "Acme".into(), "name").is_err());
    }

    #[test]
    fn optional_absent_keeps_blank_clears() {
        assert_eq!(take_optional(None, Some("x".into())), Some("x".into()));
        assert_eq!(take_optional(Some(String::new()), Some("x".into())), None);
        assert_eq!(take_optional(Some(" y ".into()), None), Some("y".into()));
    }
}
