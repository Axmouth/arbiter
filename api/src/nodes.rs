use axum::extract::State;
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::jwt::{AdminRequired, Claims};
use crate::extractors::ValidatedPath;
use crate::responses::{ApiResponse, NodeKeyResponse};
use crate::state::AppState;

/// Approving a keyholder grants it the cluster KEK (it can then decrypt every secret), so
/// node-key management is system-admin only (scope `None`), not per-tenant.
fn is_system_admin(claims: &Claims) -> bool {
    claims.scope().is_none()
}

const FORBIDDEN_MSG: &str = "only a system admin may manage node keys";

#[utoipa::path(
    get,
    path = "/node-keys",
    responses(
        (status = 200, body = ApiResponse<Vec<NodeKeyResponse>>),
        (status = 403, description = "System admin only")
    )
)]
#[axum::debug_handler]
pub async fn list_node_keys(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
) -> Result<ApiResponse<Vec<NodeKeyResponse>>, StatusCode> {
    if !is_system_admin(&claims) {
        return Ok(ApiResponse::error(StatusCode::FORBIDDEN, "forbidden", FORBIDDEN_MSG));
    }
    match state.store.list_node_keys().await {
        Ok(keys) => Ok(ApiResponse::ok(
            keys.into_iter().map(NodeKeyResponse::from).collect(),
            StatusCode::OK,
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

async fn set_status(
    state: &AppState,
    claims: &Claims,
    node_id: Uuid,
    status: &str,
) -> ApiResponse<()> {
    if !is_system_admin(claims) {
        return ApiResponse::error(StatusCode::FORBIDDEN, "forbidden", FORBIDDEN_MSG);
    }
    match state.store.set_node_key_status(node_id, status).await {
        Ok(()) => ApiResponse::ok((), StatusCode::OK),
        Err(e) => ApiResponse::error(StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string()),
    }
}

#[utoipa::path(
    post,
    path = "/node-keys/{node_id}/approve",
    responses(
        (status = 200, description = "Node approved to hold the KEK"),
        (status = 403, description = "System admin only")
    )
)]
#[axum::debug_handler]
pub async fn approve_node(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
    ValidatedPath(node_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    Ok(set_status(&state, &claims, node_id, "approved").await)
}

#[utoipa::path(
    post,
    path = "/node-keys/{node_id}/revoke",
    responses(
        (status = 200, description = "Node un-approved (stops future KEK sealing)"),
        (status = 403, description = "System admin only")
    )
)]
#[axum::debug_handler]
pub async fn revoke_node(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
    ValidatedPath(node_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    // Note: this only stops future sealing (e.g. of the next rotated KEK); it does not
    // remove a share already held. Full revocation requires a KEK rotation (see SECRETS.md).
    Ok(set_status(&state, &claims, node_id, "pending").await)
}

#[utoipa::path(
    delete,
    path = "/node-keys/{node_id}",
    responses(
        (status = 200, description = "Node evicted (dropped from the approved set)"),
        (status = 403, description = "System admin only")
    )
)]
#[axum::debug_handler]
pub async fn evict_node(
    State(state): State<AppState>,
    AdminRequired(claims): AdminRequired,
    ValidatedPath(node_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    // Evict a permanently-dead node: it drops out of the approved set so it no longer blocks
    // a rotation's ack barrier (a dead node would otherwise never ack the new version). The
    // next rotation deletes its retired shares (no key hoarding).
    Ok(set_status(&state, &claims, node_id, "evicted").await)
}
