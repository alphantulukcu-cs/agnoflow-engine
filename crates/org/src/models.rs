use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::FromRow;
use uuid::Uuid;

/// Tenant (kurum) kaydı — kimlik + kurumsal metadata + marka varlığı referansları.
///
/// Varlıkların (logo/favicon) BAYT'ları burada değil, WFD JSON ile aynı
/// tenant-prefixli object storage'da durur; bu kayıt yalnız storage anahtarını,
/// mime'ı ve son güncelleme zamanını taşır.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Orgtnt {
    pub orgtnt_id: Uuid,
    pub name: String,
    pub code: String,
    pub display_name: Option<String>,
    /// `#RRGGBB` (DB CHECK ile garanti).
    pub brand_color: Option<String>,
    pub legal_name: Option<String>,
    /// Vergi kimlik no (VKN/TCKN).
    pub tax_no: Option<String>,
    pub tax_office: Option<String>,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub website: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    /// ISO 3166-1 alpha-2.
    pub country: Option<String>,
    /// IANA saat dilimi — NOT NULL (varsayılan `Europe/Istanbul`).
    pub timezone: String,
    /// `tr` / `en-US` biçimi — NOT NULL (varsayılan `tr`).
    pub locale: String,
    /// ISO 4217 — NOT NULL (varsayılan `TRY`).
    pub currency: String,
    /// ERP/CRM eşleştirme anahtarı — kurulum genelinde tekil.
    pub external_id: Option<String>,
    pub logo_key: Option<String>,
    pub logo_mime: Option<String>,
    pub logo_updated_at: Option<DateTime<Utc>>,
    pub favicon_key: Option<String>,
    pub favicon_mime: Option<String>,
    pub favicon_updated_at: Option<DateTime<Utc>>,
    /// Şema değişmeden eklenebilen tercihler; her zaman JSON object.
    pub settings: serde_json::Value,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Tenant'ın marka varlığı slotları. Her ikisi de storage'da `logo/` dizininde
/// yaşar: `{orgtnt_id}/logo/logo.<ext>`, `{orgtnt_id}/logo/favicon.<ext>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrandAsset {
    Logo,
    Favicon,
}

impl BrandAsset {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "logo" => Some(Self::Logo),
            "favicon" => Some(Self::Favicon),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
        }
    }
}

impl Orgtnt {
    /// Slot için `(storage anahtarı, mime)` — varlık yüklenmemişse `None`.
    pub fn asset(&self, slot: BrandAsset) -> Option<(&str, &str)> {
        let (key, mime) = match slot {
            BrandAsset::Logo => (&self.logo_key, &self.logo_mime),
            BrandAsset::Favicon => (&self.favicon_key, &self.favicon_mime),
        };
        // DB CHECK ikisini birlikte tutar; yine de ikisini de talep ediyoruz.
        Some((key.as_deref()?, mime.as_deref()?))
    }
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgt {
    pub orgt_id: Uuid,
    pub orgtnt_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub is_default: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Orgu {
    pub orgu_id: Uuid,
    pub orgt_id: Uuid,
    pub orgtnt_id: Uuid,
    pub parent_orgu_id: Option<Uuid>,
    pub path: String,
    pub orgu_type: serde_json::Value,
    pub name: String,
    pub metadata: Option<serde_json::Value>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct User {
    pub u_id: Uuid,
    pub orgtnt_id: Uuid,
    pub username: String,
    pub full_name: String,
    pub email: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct Role {
    pub r_id: Uuid,
    pub orgtnt_id: Uuid,
    pub name: String,
    pub display_name: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
}

/// `org.p` — tenant'ın atomik yetki havuzundan bir satır.
///
/// `code` numara ("1043") da olabilir, isim (`KREDI_ONAY`) da; agnoflow anlamını
/// BİLMEZ. Yeniden adlandırılabilir olması gerektiği için atamalar `p_id`'ye bağlıdır
/// (`org.r`'de `r_id` ile aynı gerekçe).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Permission {
    pub p_id: Uuid,
    pub orgtnt_id: Uuid,
    pub code: String,
    pub display_name: String,
    pub description: Option<String>,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// `org.p` ters sorgusu: bir yetki hangi rollerde ve kaç kullanıcıya ulaşıyor.
/// Yetkiyi havuzdan kaldırmadan önce yöneticinin görmesi gereken sayı.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PermissionRoleUsage {
    pub r_id: Uuid,
    pub role_name: String,
    pub user_count: i64,
}

/// `org.up` satırı + kataloğundan ad — kişisel ıskarta listesi (T‑A2).
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct PermissionException {
    pub up_id: Uuid,
    pub p_id: Uuid,
    pub code: String,
    pub display_name: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
}

/// `org.orgtnt_api_key` — tenant kapsamlı SALT OKUMA anahtarı (`/ext` ağacı).
///
/// `key_hash` serialize EDİLMEZ: liste ucu anahtar hakkında hiçbir sır sızdırmaz.
/// Düz metin yalnız yaratılışta bir kez döner ve hiçbir yerde saklanmaz.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct TenantApiKey {
    pub key_id: Uuid,
    pub orgtnt_id: Uuid,
    pub name: String,
    pub prefix: String,
    #[serde(skip)]
    pub key_hash: String,
    pub is_active: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct OrguTypeDef {
    pub type_id: Uuid,
    pub orgtnt_id: Uuid,
    pub key: String,
    pub display_name: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserOrgu {
    pub u_orgu_id: Uuid,
    pub orgtnt_id: Uuid,
    pub u_id: Uuid,
    pub orgu_id: Uuid,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct UserRole {
    pub ur_id: Uuid,
    pub orgtnt_id: Uuid,
    pub u_id: Uuid,
    pub r_id: Uuid,
    pub role_name: String,
    pub orgu_id: Option<Uuid>,
    pub orgu_scope: Option<String>,
    pub ur_type: String,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Minimal org unit view used by wfe-core via OrgPort.
#[derive(Debug, Clone, Serialize)]
pub struct OrgUnit {
    pub orgu_id: Uuid,
    pub orgu_type: serde_json::Value,
    pub path: String,
}

impl From<Orgu> for OrgUnit {
    fn from(o: Orgu) -> Self {
        Self {
            orgu_id: o.orgu_id,
            orgu_type: o.orgu_type,
            path: o.path,
        }
    }
}

/// Madde 6: vekalet/delegasyon kaydı. `grantee` bir CandidateActor JSONB'dir
/// (kişi {c_u:[...]} veya havuz {c_orgu, c_r:[...]}); alıcı eşleşmesi wfe-core
/// matcher'ında yapılır. Koltuk = (seat_orgu_id, seat_role).
#[derive(Debug, Serialize, FromRow)]
pub struct Delegation {
    pub delegation_id: Uuid,
    pub orgtnt_id: Uuid,
    pub delegator_user_id: Uuid,
    pub seat_orgu_id: Uuid,
    pub seat_role: String,
    pub grantee: serde_json::Value,
    pub valid_from: DateTime<Utc>,
    pub valid_to: DateTime<Utc>,
    pub active: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}
