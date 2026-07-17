use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use wfe_core::EngineError;

#[derive(Debug)]
pub struct AppError(pub String, pub StatusCode);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.1, Json(json!({"error": self.0}))).into_response()
    }
}

impl From<EngineError> for AppError {
    fn from(e: EngineError) -> Self {
        let status = match &e {
            EngineError::PermissionDenied(_) => StatusCode::FORBIDDEN,
            EngineError::TransitionNotFound(_) => StatusCode::BAD_REQUEST,
            EngineError::WfeTerminal => StatusCode::CONFLICT,
            EngineError::WfeExpired => StatusCode::CONFLICT,
            EngineError::StartNotEligible => StatusCode::FORBIDDEN,
            EngineError::NotClaimed => StatusCode::FORBIDDEN,
            EngineError::NotOwner => StatusCode::FORBIDDEN,
            EngineError::Unauthorized => StatusCode::FORBIDDEN,
            EngineError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            EngineError::UnsupportedWfdVersion(_) => StatusCode::UNPROCESSABLE_ENTITY,
            EngineError::InvalidWfd(_) => StatusCode::UNPROCESSABLE_ENTITY,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        AppError(e.to_string(), status)
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
