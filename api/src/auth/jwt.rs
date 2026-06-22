use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
};
use chrono::{Duration, Utc};
use arbiter_core::{DEFAULT_TENANT_ID, UserRole};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub sub: Uuid,
    pub role: String,
    /// The caller's tenant: `None` = system-wide (all tenants), `Some(t)` = tenant t.
    #[serde(default)]
    pub tenant_id: Option<Uuid>,
    pub exp: usize,
}

impl Claims {
    /// Tenant scope for list/get queries: `None` = system caller (all tenants).
    pub fn scope(&self) -> Option<Uuid> {
        self.tenant_id
    }

    /// The tenant a created resource belongs to (system callers create in the default).
    pub fn create_tenant(&self) -> Uuid {
        self.tenant_id.unwrap_or(DEFAULT_TENANT_ID)
    }
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

pub fn encode_jwt(user_id: Uuid, role: &str, tenant_id: Option<Uuid>, keys: &JwtKeys) -> String {
    let exp = (Utc::now() + Duration::hours(12)).timestamp() as usize;

    let claims = Claims {
        sub: user_id,
        role: role.to_string(),
        tenant_id,
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
