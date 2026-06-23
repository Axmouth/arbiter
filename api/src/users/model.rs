use arbiter_core::UserRole;
use serde::Deserialize;
use ts_rs::TS;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, TS, ToSchema)]
#[ts(export)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: UserRole,
    /// The tenant to place the user in. Honored only for a system admin (`None` =
    /// a system-scoped user). A tenant admin always creates within their own tenant.
    pub tenant_id: Option<Uuid>,
}

#[derive(Deserialize, TS, ToSchema)]
#[ts(export)]
pub struct UpdateUserRequest {
    pub username: Option<String>,
    pub password: Option<String>,
    pub role: Option<UserRole>,
}
