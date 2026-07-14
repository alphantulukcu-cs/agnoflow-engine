// workflow-engine/crates/server/src/routes/portal/jwt.rs

use crate::{error::AppError, state::AppState};
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct PortalClaims {
    pub sub: String, // user_id
    pub orgu_id: String,
    pub role: String,
    pub orgtnt_id: String,
    pub exp: usize,
}

#[derive(Debug, Clone)]
pub struct PortalActor {
    pub user_id: Uuid,
    pub orgu_id: Uuid,
    pub role: String,
    pub orgtnt_id: Uuid,
}

pub fn encode_jwt(secret: &str, actor: &PortalActor, ttl_hours: u64) -> Result<String, AppError> {
    let exp = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + ttl_hours * 3600) as usize;

    let claims = PortalClaims {
        sub: actor.user_id.to_string(),
        orgu_id: actor.orgu_id.to_string(),
        role: actor.role.clone(),
        orgtnt_id: actor.orgtnt_id.to_string(),
        exp,
    };

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| {
        AppError(
            format!("JWT encode: {e}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
    })
}

pub fn decode_jwt(secret: &str, token: &str) -> Result<PortalActor, AppError> {
    let data = decode::<PortalClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| AppError("Invalid or expired token".into(), StatusCode::UNAUTHORIZED))?;

    let c = data.claims;
    Ok(PortalActor {
        user_id: Uuid::parse_str(&c.sub)
            .map_err(|_| AppError("Bad token: sub".into(), StatusCode::UNAUTHORIZED))?,
        orgu_id: Uuid::parse_str(&c.orgu_id)
            .map_err(|_| AppError("Bad token: orgu_id".into(), StatusCode::UNAUTHORIZED))?,
        role: c.role,
        orgtnt_id: Uuid::parse_str(&c.orgtnt_id)
            .map_err(|_| AppError("Bad token: orgtnt_id".into(), StatusCode::UNAUTHORIZED))?,
    })
}

#[async_trait]
impl FromRequestParts<AppState> for PortalActor {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| {
                AppError(
                    "Authorization: Bearer <token> required".into(),
                    StatusCode::UNAUTHORIZED,
                )
            })?;

        decode_jwt(&state.cfg.jwt_secret, token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn test_actor() -> PortalActor {
        PortalActor {
            user_id: Uuid::new_v4(),
            orgu_id: Uuid::new_v4(),
            role: "clerk".into(),
            orgtnt_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn roundtrip_encode_decode() {
        let secret = "test-secret-12345678901234567890";
        let actor = test_actor();
        let token = encode_jwt(secret, &actor, 1).expect("encode");
        let decoded = decode_jwt(secret, &token).expect("decode");
        assert_eq!(decoded.user_id, actor.user_id);
        assert_eq!(decoded.orgu_id, actor.orgu_id);
        assert_eq!(decoded.role, actor.role);
        assert_eq!(decoded.orgtnt_id, actor.orgtnt_id);
    }

    #[test]
    fn wrong_secret_fails() {
        let actor = test_actor();
        let token = encode_jwt("secret-a-1234567890123456789", &actor, 1).expect("encode");
        let result = decode_jwt("secret-b-1234567890123456789", &token);
        assert!(result.is_err());
    }
}
