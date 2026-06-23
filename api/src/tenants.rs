use axum::extract::State;
use axum::http::StatusCode;
use arbiter_core::Tenant;

use crate::auth::jwt::AdminRequired;
use crate::extractors::ValidatedJson;
use crate::requests::CreateTenantRequest;
use crate::responses::ApiResponse;
use crate::state::AppState;

#[utoipa::path(
    post,
    path = "/tenants",
    request_body = CreateTenantRequest,
    responses(
        (status = 201, body = ApiResponse<Tenant>),
        (status = 403, description = "Only a system admin may create tenants")
    )
)]
#[axum::debug_handler]
pub async fn create_tenant(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
    ValidatedJson(req): ValidatedJson<CreateTenantRequest>,
) -> Result<ApiResponse<Tenant>, StatusCode> {
    // Creating a tenant is a platform operation: system admin only (scope None).
    if claims.scope().is_some() {
        return Ok(ApiResponse::error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "only a system admin may create tenants",
        ));
    }
    match state.store.create_tenant(&req.name).await {
        Ok(tenant) => Ok(ApiResponse::ok(tenant, StatusCode::CREATED)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/tenants",
    responses(
        (status = 200, body = ApiResponse<Vec<Tenant>>)
    )
)]
#[axum::debug_handler]
pub async fn list_tenants(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
) -> Result<ApiResponse<Vec<Tenant>>, StatusCode> {
    // A system admin sees every tenant; a tenant admin sees only their own.
    match claims.scope() {
        None => match state.store.list_tenants().await {
            Ok(tenants) => Ok(ApiResponse::ok(tenants, StatusCode::OK)),
            Err(e) => Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_error",
                e.to_string(),
            )),
        },
        Some(id) => match state.store.get_tenant(id).await {
            Ok(Some(tenant)) => Ok(ApiResponse::ok(vec![tenant], StatusCode::OK)),
            Ok(None) => Ok(ApiResponse::ok(vec![], StatusCode::OK)),
            Err(e) => Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_error",
                e.to_string(),
            )),
        },
    }
}
