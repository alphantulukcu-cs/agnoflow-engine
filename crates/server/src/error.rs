use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use wfe_core::EngineError;

#[derive(Debug)]
pub struct AppError {
    pub message: String,
    pub status: StatusCode,
    /// WOR-62: makine-okunur hata kodu — yalnız conflict (409) sınıfı için
    /// doldurulur, gövdede `code` alanı olarak çıkar. Portal ayrımı bu koddan
    /// yapar, hata METNİNİ parse ETMEZ (metin i18n/refactor ile değişebilir).
    pub code: Option<&'static str>,
    /// Tek istekli başlatmada (2026-08-11) ÇOK dosyanın reddini anlatmak için: gövdeye
    /// `items` alanı olarak eklenir. Tek `error` metni "hangi belge neden reddedildi"
    /// sorusunu N dosya için cevaplayamıyordu; istemci bunu slot bazında gösterir.
    pub items: Option<serde_json::Value>,
}

/// Mevcut ~120 çağrı yerinin `AppError(mesaj, status)` biçimini bozmadan üçüncü
/// alanı ekleyebilmek için tuple-struct yerine aynı isimli yapıcı fonksiyon
/// (tip ve değer namespace'leri Rust'ta ayrıdır; `AppError` tipi ve `AppError(..)`
/// çağrısı birlikte yaşar). Kod taşıyan hatalar `AppError { code: Some(..), .. }`
/// struct-literal'i ile kurulur — bugün tek üretici `From<EngineError>`'dır.
///
/// Parametre `String`'dir (`impl Into<String>` DEĞİL): mevcut çağrı yerleri
/// `"...".into()` yazıyor, generic parametre bu `.into()`'ları belirsizleştirir.
#[allow(non_snake_case)]
pub fn AppError(message: String, status: StatusCode) -> AppError {
    AppError {
        message,
        status,
        code: None,
        items: None,
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Gövde şekli: {"error": "<mesaj>"} + conflict sınıfında {"code": "<kod>"}.
        // `error` alanı GERİYE UYUMLU kalır — `code` yalnızca EKLENİR.
        let mut body = match self.code {
            Some(code) => json!({"error": self.message, "code": code}),
            None => json!({"error": self.message}),
        };
        // `items` de yalnızca EKLENİR — alanı tanımayan istemci `error`u okumaya devam eder.
        if let (Some(items), Some(obj)) = (self.items, body.as_object_mut()) {
            obj.insert("items".into(), items);
        }
        (self.status, Json(body)).into_response()
    }
}

impl From<EngineError> for AppError {
    fn from(e: EngineError) -> Self {
        let status = match &e {
            EngineError::PermissionDenied(_) => StatusCode::FORBIDDEN,
            EngineError::TransitionNotFound(_) => StatusCode::BAD_REQUEST,
            EngineError::WfeTerminal => StatusCode::CONFLICT,
            EngineError::WfeExpired => StatusCode::CONFLICT,
            // WOR-31 T4: paralel modda aksiyon ≥2 aktif kolla eşleşip node hint
            // verilmediğinde — mesajda aday kol node'ları taşınır (bkz. EngineError Display).
            EngineError::AmbiguousAction { .. } => StatusCode::CONFLICT,
            // WOR-31 T4: adapter'ın FOR UPDATE + CAS uyumsuzluğu — executor
            // retry-edilebilir kind'lar için 3 kez dener (bkz. WfeExecutor::apply);
            // buraya sızan ya tükenmiş ya da retry-edilemez bir verdikttir.
            EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::StartNotEligible => StatusCode::FORBIDDEN,
            EngineError::NotClaimed => StatusCode::FORBIDDEN,
            EngineError::NotOwner => StatusCode::FORBIDDEN,
            EngineError::Unauthorized => StatusCode::FORBIDDEN,
            EngineError::TargetNotEligible => StatusCode::BAD_REQUEST,
            EngineError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            EngineError::UnsupportedWfdVersion(_) => StatusCode::UNPROCESSABLE_ENTITY,
            EngineError::InvalidWfd(_) => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // WOR-62: 409 sınıfının TAMAMI makine-okunur bir `code` taşır — portal
        // "collapse oldu" / "kol taşındı" / "başkası aldı" / "aksiyon belirsiz"
        // ayrımını buradan yapar. WOR-65'in ekleyeceği stale-revizyon reddi de
        // aynı namespace'e (`conflict.*`) düşer: yeni `ConflictKind` varyantı
        // eklemek burada DEĞİŞİKLİK GEREKTİRMEZ.
        let code = match &e {
            EngineError::Conflict(kind) => Some(kind.code()),
            EngineError::AmbiguousAction { .. } => Some("conflict.ambiguous_action"),
            EngineError::WfeTerminal => Some("conflict.terminal"),
            EngineError::WfeExpired => Some("conflict.expired"),
            _ => None,
        };
        AppError {
            message: e.to_string(),
            status,
            code,
            items: None,
        }
    }
}

impl From<wf_org::error::OrgError> for AppError {
    fn from(e: wf_org::error::OrgError) -> Self {
        // Şema kısıtı ihlali kullanıcı girdisinden doğar: 500 + çıplak SQL metni
        // yerine 400/409 + okunabilir mesaj. Kısıt ADI üzerinden eşlenir; mesajı
        // parse ETMEZ (Postgres metni sürümle değişir).
        if let wf_org::error::OrgError::Database(sqlx::Error::Database(dbe)) = &e {
            if let Some(app) = from_constraint(dbe.as_ref()) {
                return app;
            }
        }
        // Conflict'in taşıdığı metin makine kodudur; istemci ayrımı `code` alanından
        // yapar (WOR-62 duruşu: hata METNİ i18n/refactor ile değişebilir).
        if let wf_org::error::OrgError::Conflict(kind) = &e {
            let (message, code) = org_conflict(kind);
            return AppError {
                message: message.to_string(),
                status: StatusCode::CONFLICT,
                code,
                items: None,
            };
        }
        // 404'ler de kod taşır: istemci "hangi kaynak yok" ayrımını hata METNİNDEN
        // yapmak zorunda kalmasın. Beyaz liste dışı kaynak kodsuz döner.
        if let wf_org::error::OrgError::NotFound(resource) = &e {
            return AppError {
                message: e.to_string(),
                status: StatusCode::NOT_FOUND,
                code: not_found_code(resource),
                items: None,
            };
        }
        let status = match &e {
            wf_org::error::OrgError::BadRequest(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(e.to_string(), status)
    }
}

/// `OrgError::NotFound`'un taşıdığı kaynak adını makine koduna bağlar.
fn not_found_code(resource: &str) -> Option<&'static str> {
    match resource {
        "permission" => Some("permission.not_found"),
        "user" => Some("user.not_found"),
        "role" => Some("role.not_found"),
        "api key" => Some("api_key.not_found"),
        _ => None,
    }
}

/// `OrgError::Conflict` metnini insan mesajı + `&'static str` koduna bağlar.
/// Beyaz liste: gövdeye yalnız BİLDİĞİMİZ kodlar çıkar, rastgele bir hata metni
/// `code` alanına sızmaz. Bilinmeyen çakışma kodsuz ve genel mesajla döner —
/// makine kodunun `error` alanına düşüp kullanıcıya gösterilmesi yerine.
fn org_conflict(kind: &str) -> (&'static str, Option<&'static str>) {
    match kind {
        "permission.in_use" => (
            "yetki bir rolde ya da kişisel ıskartada kullanılıyor; \
             silmek yerine is_active=false yapın",
            Some("permission.in_use"),
        ),
        _ => ("kaynağın mevcut durumu bu işlemi kabul etmiyor", None),
    }
}

/// Postgres kısıt ihlalini istemci hatasına çevirir.
/// SQLSTATE: `23514` check, `23505` unique, `23503` foreign key.
fn from_constraint(dbe: &dyn sqlx::error::DatabaseError) -> Option<AppError> {
    let sqlstate = dbe.code()?.to_string();
    let constraint = dbe.constraint().unwrap_or("");

    // Bilinen kısıtlar için alan bazlı mesaj.
    let known = match constraint {
        "orgtnt_brand_color_hex" => Some("brand_color '#RRGGBB' biçiminde olmalı"),
        "orgtnt_country_iso2" => Some("country ISO 3166-1 alpha-2 (iki harf) olmalı"),
        "orgtnt_currency_iso4217" => Some("currency ISO 4217 (üç harf) olmalı"),
        "orgtnt_locale_bcp47_lite" => Some("locale 'tr' ya da 'tr-TR' biçiminde olmalı"),
        "orgtnt_contact_email_shape" => Some("contact_email geçerli bir e-posta olmalı"),
        "orgtnt_settings_is_object" => Some("settings bir JSON object olmalı"),
        "orgtnt_no_blank_text" => Some("alan boş metin olamaz"),
        "orgtnt_external_id_unique" => Some("external_id başka bir tenant'ta kullanılıyor"),
        "orgtnt_code_key" => Some("code başka bir tenant'ta kullanılıyor"),
        "p_code_format" => Some(
            "permission kodu yalnız ASCII harf/rakam ve . _ : - içerebilir (en çok 128 karakter)",
        ),
        "p_code_unique" => Some("bu permission kodu tenant'ta zaten tanımlı"),
        _ => None,
    };

    // Makine-okunur kod: istemci mesajı parse etmesin. Yalnız beyaz listedeki
    // kısıtlar kod taşır.
    let code = match constraint {
        "p_code_format" => Some("permission.code_format"),
        "p_code_unique" => Some("permission.code_conflict"),
        _ => None,
    };

    let (message, status) = match sqlstate.as_str() {
        "23514" => (
            known.unwrap_or("gönderilen değer şema kısıtını ihlal ediyor"),
            StatusCode::BAD_REQUEST,
        ),
        "23505" => (known.unwrap_or("kayıt zaten mevcut"), StatusCode::CONFLICT),
        _ => return None,
    };
    Some(AppError {
        message: message.to_string(),
        status,
        code,
        items: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wf_org::error::OrgError;

    /// 404'ler de makine-okunur kod taşır: istemci "hangi kaynak yok" ayrımını
    /// hata METNİNDEN yapmak zorunda kalmasın (WOR-62 duruşu).
    #[test]
    fn not_found_carries_resource_code() {
        let app: AppError = OrgError::NotFound("permission".into()).into();
        assert_eq!(app.status, StatusCode::NOT_FOUND);
        assert_eq!(app.code, Some("permission.not_found"));
    }

    #[test]
    fn known_resources_each_get_their_own_code() {
        for (resource, expected) in [
            ("permission", "permission.not_found"),
            ("user", "user.not_found"),
            ("role", "role.not_found"),
            ("api key", "api_key.not_found"),
        ] {
            let app: AppError = OrgError::NotFound(resource.into()).into();
            assert_eq!(app.code, Some(expected), "kaynak: {resource}");
        }
    }

    /// Beyaz liste dışı kaynak KOD TAŞIMAZ — gövdeye rastgele metin sızmasın.
    #[test]
    fn unknown_resource_has_no_code() {
        let app: AppError = OrgError::NotFound("orgu".into()).into();
        assert_eq!(app.status, StatusCode::NOT_FOUND);
        assert_eq!(app.code, None);
    }

    #[test]
    fn conflict_maps_to_human_message_and_code() {
        let app: AppError = OrgError::Conflict("permission.in_use".into()).into();
        assert_eq!(app.status, StatusCode::CONFLICT);
        assert_eq!(app.code, Some("permission.in_use"));
        assert!(
            !app.message.contains("permission.in_use"),
            "kullanıcıya makine kodu değil insan mesajı gösterilir: {}",
            app.message
        );
    }

    /// Bilinmeyen çakışma kodsuz ve genel mesajla döner.
    #[test]
    fn unknown_conflict_has_no_code() {
        let app: AppError = OrgError::Conflict("bir.sey".into()).into();
        assert_eq!(app.code, None);
        assert!(!app.message.contains("bir.sey"));
    }
}
