use axum::extract::State;
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::jwt::{AdminRequired, AuthClaims};
use crate::extractors::{ValidatedJson, ValidatedPath};
use crate::requests::CreateSecretRequest;
use crate::responses::{ApiResponse, RotateKekResponse, SecretMetaResponse};
use crate::state::AppState;

#[utoipa::path(
    post,
    path = "/secrets",
    request_body = CreateSecretRequest,
    responses(
        (status = 201, body = ApiResponse<SecretMetaResponse>, description = "Secret created (value never returned)"),
        (status = 400, description = "Invalid secret name"),
        (status = 503, description = "This node cannot create secrets (no key)")
    )
)]
#[axum::debug_handler]
pub async fn create_secret(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedJson(req): ValidatedJson<CreateSecretRequest>,
) -> Result<ApiResponse<SecretMetaResponse>, StatusCode> {
    if req.name.trim().is_empty() {
        return Ok(ApiResponse::error(
            StatusCode::BAD_REQUEST,
            "invalid_name",
            "secret name must not be empty",
        ));
    }

    // Creating a secret needs the KEK, held only by a node running with one. A node
    // without it can still list/delete secret metadata, just not encrypt new values.
    let Some(secrets) = state.secrets.as_ref() else {
        return Ok(ApiResponse::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_key",
            "this node cannot create secrets (no encryption key)",
        ));
    };

    let tenant = claims.create_tenant();
    let id = match secrets.set_secret(tenant, &req.name, req.value.as_bytes()).await {
        Ok(id) => id,
        Err(e) => {
            return Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "secret_error",
                e.to_string(),
            ));
        }
    };

    // Return metadata only, fetched back so timestamps are accurate. Never the value.
    match state.store.get_secret(id, claims.scope()).await {
        Ok(Some(s)) => Ok(ApiResponse::ok(
            SecretMetaResponse {
                id: s.id,
                name: s.name,
                kek_version: s.kek_version,
                created_at: s.created_at,
                updated_at: s.updated_at,
            },
            StatusCode::CREATED,
        )),
        Ok(None) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            "secret vanished after create",
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/secrets",
    responses(
        (status = 200, body = ApiResponse<Vec<SecretMetaResponse>>, description = "Secret metadata (no values)")
    )
)]
#[axum::debug_handler]
pub async fn list_secrets(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
) -> Result<ApiResponse<Vec<SecretMetaResponse>>, StatusCode> {
    match state.store.list_secret_names(claims.scope()).await {
        Ok(metas) => Ok(ApiResponse::ok(
            metas.into_iter().map(SecretMetaResponse::from).collect(),
            StatusCode::OK,
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/secrets/rotate",
    responses(
        (status = 200, body = ApiResponse<RotateKekResponse>, description = "KEK rotated"),
        (status = 403, description = "System admin only"),
        (status = 503, description = "This node holds no key to rotate")
    )
)]
#[axum::debug_handler]
pub async fn rotate_kek(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
) -> Result<ApiResponse<RotateKekResponse>, StatusCode> {
    // Rotation re-keys every tenant's secrets and changes who can decrypt them, so it is
    // system-admin only (scope None), like node-key approval.
    if claims.scope().is_some() {
        return Ok(ApiResponse::error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only a system admin may rotate the KEK",
        ));
    }
    let Some(secrets) = state.secrets.as_ref() else {
        return Ok(ApiResponse::error(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_key",
            "this node holds no key to rotate",
        ));
    };
    match secrets.rotate_kek().await {
        Ok(version) => Ok(ApiResponse::ok(
            RotateKekResponse { kek_version: version },
            StatusCode::OK,
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rotate_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    delete,
    path = "/secrets/{id}",
    responses(
        (status = 204, description = "Secret deleted"),
        (status = 404, description = "Secret not found")
    )
)]
#[axum::debug_handler]
pub async fn delete_secret(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    // Scope guard: a tenant caller may only delete a secret in its own tenant.
    match state.store.get_secret(id, claims.scope()).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Ok(ApiResponse::error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("secret {id} not found"),
            ));
        }
        Err(e) => {
            return Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_error",
                e.to_string(),
            ));
        }
    }
    match state.store.delete_secret(id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::NO_CONTENT)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}
