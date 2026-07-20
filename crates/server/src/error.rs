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
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Gövde şekli: {"error": "<mesaj>"} + conflict sınıfında {"code": "<kod>"}.
        // `error` alanı GERİYE UYUMLU kalır — `code` yalnızca EKLENİR.
        let body = match self.code {
            Some(code) => json!({"error": self.message, "code": code}),
            None => json!({"error": self.message}),
        };
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
        }
    }
}

impl From<wf_org::error::OrgError> for AppError {
    fn from(e: wf_org::error::OrgError) -> Self {
        let status = match &e {
            wf_org::error::OrgError::NotFound(_) => StatusCode::NOT_FOUND,
            wf_org::error::OrgError::BadRequest(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(e.to_string(), status)
    }
}
