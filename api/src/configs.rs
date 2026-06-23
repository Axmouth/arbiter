use axum::extract::State;
use axum::http::StatusCode;
use arbiter_core::SharedDbConfig;
use uuid::Uuid;

use crate::auth::jwt::AuthClaims;
use crate::extractors::{ValidatedJson, ValidatedPath};
use crate::requests::{CreateDbConfigRequest, UpdateDbConfigRequest};
use crate::responses::ApiResponse;
use crate::state::AppState;

/// Returns an error response if the config is outside the caller's scope (or missing), so
/// a tenant caller cannot read or mutate another tenant's config.
async fn config_scope_error(
    state: &AppState,
    id: Uuid,
    scope: Option<Uuid>,
) -> Option<(StatusCode, &'static str, String)> {
    match state.store.get_db_config(id, scope).await {
        Ok(Some(_)) => None,
        Ok(None) => Some((StatusCode::NOT_FOUND, "not_found", format!("config {id} not found"))),
        Err(e) => Some((StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())),
    }
}

#[utoipa::path(
    post,
    path = "/db-configs",
    request_body = CreateDbConfigRequest,
    responses(
        (status = 201, body = ApiResponse<SharedDbConfig>)
    )
)]
#[axum::debug_handler]
pub async fn create_db_config(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedJson(req): ValidatedJson<CreateDbConfigRequest>,
) -> Result<ApiResponse<SharedDbConfig>, StatusCode> {
    match state
        .store
        .create_db_config(
            claims.create_tenant(),
            req.engine,
            &req.name,
            &req.host,
            req.port,
            &req.username,
            &req.password_secret,
            &req.database,
        )
        .await
    {
        Ok(cfg) => Ok(ApiResponse::ok(cfg, StatusCode::CREATED)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/db-configs",
    responses(
        (status = 200, body = ApiResponse<Vec<SharedDbConfig>>)
    )
)]
#[axum::debug_handler]
pub async fn list_db_configs(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
) -> Result<ApiResponse<Vec<SharedDbConfig>>, StatusCode> {
    match state.store.list_db_configs(claims.scope()).await {
        Ok(configs) => Ok(ApiResponse::ok(configs, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/db-configs/{id}",
    responses(
        (status = 200, body = ApiResponse<SharedDbConfig>),
        (status = 404, description = "Config not found")
    )
)]
#[axum::debug_handler]
pub async fn get_db_config(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<SharedDbConfig>, StatusCode> {
    match state.store.get_db_config(id, claims.scope()).await {
        Ok(Some(cfg)) => Ok(ApiResponse::ok(cfg, StatusCode::OK)),
        Ok(None) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("config {id} not found"),
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    patch,
    path = "/db-configs/{id}",
    request_body = UpdateDbConfigRequest,
    responses(
        (status = 200, body = ApiResponse<SharedDbConfig>),
        (status = 404, description = "Config not found")
    )
)]
#[axum::debug_handler]
pub async fn update_db_config(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
    ValidatedJson(req): ValidatedJson<UpdateDbConfigRequest>,
) -> Result<ApiResponse<SharedDbConfig>, StatusCode> {
    if let Some((sc, code, msg)) = config_scope_error(&state, id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
    match state
        .store
        .update_db_config(
            id,
            req.name.as_deref(),
            req.host.as_deref(),
            req.port,
            req.username.as_deref(),
            req.password_secret.as_deref(),
            req.database.as_deref(),
        )
        .await
    {
        Ok(cfg) => Ok(ApiResponse::ok(cfg, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    delete,
    path = "/db-configs/{id}",
    responses(
        (status = 204, description = "Config deleted"),
        (status = 404, description = "Config not found")
    )
)]
#[axum::debug_handler]
pub async fn delete_db_config(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    if let Some((sc, code, msg)) = config_scope_error(&state, id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
    match state.store.delete_db_config(id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::NO_CONTENT)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}
