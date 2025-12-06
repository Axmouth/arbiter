use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use chrono::{Duration, Utc};
use dromio_core::UserRole;
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: Uuid,
    pub role: String,
    pub exp: usize,
}

pub struct AuthClaims(pub Claims);

#[derive(Clone)]
pub struct JwtKeys {
    pub encoding: EncodingKey,
    pub decoding: DecodingKey,
}

impl JwtKeys {
    pub fn from_secret(secret: &str) -> Self {
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
        }
    }
}

pub fn encode_jwt(user_id: Uuid, role: &str, keys: &JwtKeys) -> String {
    let exp = (Utc::now() + Duration::hours(12)).timestamp() as usize;

    let claims = Claims {
        sub: user_id,
        role: role.to_string(),
        exp,
    };

    encode(&Header::default(), &claims, &keys.encoding).expect("JWT encode failed")
}

/// Returns Ok(Claims) or Err(401)
pub fn decode_jwt(token: &str, keys: &JwtKeys) -> Result<Claims, ()> {
    decode::<Claims>(token, &keys.decoding, &Validation::default())
        .map(|data| data.claims)
        .map_err(|_| ())
}

impl<S> FromRequestParts<S> for AuthClaims
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Claims>()
            .cloned()
            .map(AuthClaims)
            .ok_or(StatusCode::UNAUTHORIZED)
    }
}

pub struct AdminRequired(pub Claims);

impl<S> FromRequestParts<S> for AdminRequired
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        if let Some(claims) = parts.extensions.get::<Claims>() {
            if let Ok(UserRole::Admin) = claims.role.parse() {
                return Ok(AdminRequired(claims.clone()));
            }
            return Err(StatusCode::FORBIDDEN);
        }
        Err(StatusCode::UNAUTHORIZED)
    }
}
