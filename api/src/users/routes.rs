use axum::{extract::State, http::StatusCode};
use arbiter_core::User;
use tower_cookies::cookie::SameSite;
use tower_cookies::{Cookie, Cookies};
use uuid::Uuid;

use crate::auth::jwt::{AdminRequired, AuthClaims, encode_jwt};
use crate::auth::{hash_password, verify_password};
use crate::extractors::{ValidatedJson, ValidatedPath};
use crate::responses::ApiResponse;
use crate::state::AppState;
use crate::users::model::{CreateUserRequest, LoginRequest, UpdateUserRequest};

#[utoipa::path(
    post,
    path = "/login",
    responses(
        (status = 200, description = "Login Successful"),
        (status = 401, description = "Unauthorized")
    )
)]
#[axum::debug_handler]
pub async fn login(
    State(state): State<AppState>,
    cookies: Cookies,
    ValidatedJson(req): ValidatedJson<LoginRequest>,
) -> Result<ApiResponse<()>, StatusCode> {
    const FAIL_LOGIN_MESSAGE: &str = "User/Password combination not found";
    let user = if let Ok(usr) = state.store.get_user_by_username(&req.username).await {
        usr
    } else {
        return Ok(ApiResponse::error(
            StatusCode::UNAUTHORIZED,
            FAIL_LOGIN_MESSAGE,
            FAIL_LOGIN_MESSAGE,
        ));
    };

    if !verify_password(&user.password_hash, &req.password) {
        return Ok(ApiResponse::error(
            StatusCode::UNAUTHORIZED,
            FAIL_LOGIN_MESSAGE,
            FAIL_LOGIN_MESSAGE,
        ));
    }

    let token = encode_jwt(user.id, &user.role.to_string(), &state.jwt_keys);

    let mut cookie = Cookie::new("arbiter_session", token);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_expires(
        tower_cookies::cookie::time::OffsetDateTime::now_utc()
            + tower_cookies::cookie::time::Duration::hours(12),
    );
    cookies.add(cookie);

    Ok(ApiResponse::ok((), StatusCode::OK))
}

#[utoipa::path(
    get,
    path = "/users",
    responses(
        (status = 200, body = ApiResponse<Vec<User>>),
        (status = 403, description = "Forbidden"),
    )
)]
#[axum::debug_handler]
pub async fn list_users(
    State(state): State<AppState>,
    AdminRequired(_claims): AdminRequired,
) -> Result<ApiResponse<Vec<User>>, StatusCode> {
    let users = match state.store.list_users().await {
        Ok(usrs) => usrs,
        Err(err) => {
            return Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Error fetching users",
                err.to_string(),
            ));
        }
    };

    Ok(ApiResponse::ok(users, StatusCode::OK))
}

#[utoipa::path(
    get,
    path="/users/{id}",
    responses(
        (status = 200, body = ApiResponse<User>),
        (status = 404, description = "User not found"),
    )
)]
pub async fn get_user(
    State(state): State<AppState>,
    AdminRequired(_claims): AdminRequired,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<User>, StatusCode> {
    match state.store.get_user_by_id(id).await {
        Ok(user) => Ok(ApiResponse::ok(user, StatusCode::OK)),
        Err(_) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            "User not found",
        )),
    }
}

#[utoipa::path(
    patch,
    path="/users/{id}",
    responses(
        (status = 200, body = ApiResponse<User>),
        (status = 404, description = "User not found"),
    )
)]
pub async fn update_user(
    State(state): State<AppState>,
    AdminRequired(_claims): AdminRequired,
    ValidatedPath(id): ValidatedPath<Uuid>,
    ValidatedJson(req): ValidatedJson<UpdateUserRequest>,
) -> Result<ApiResponse<User>, StatusCode> {
    let password_hash = req.password.map(|p| hash_password(&p));

    match state
        .store
        .update_user(
            id,
            req.username.as_deref(),
            password_hash.as_deref(),
            req.role,
        )
        .await
    {
        Ok(user) => Ok(ApiResponse::ok(user, StatusCode::OK)),
        Err(_) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            "User not found",
        )),
    }
}

#[utoipa::path(
    delete,
    path="/users/{id}",
    responses(
        (status = 204, description = "User deleted successfully"),
        (status = 404, description = "User not found"),
    )
)]
pub async fn delete_user(
    State(state): State<AppState>,
    AdminRequired(_claims): AdminRequired,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.delete_user(id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::NO_CONTENT)),
        Err(_) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            "User not found",
        )),
    }
}

#[utoipa::path(
    post,
    path="/users",
    responses(
        (status = 200, description = "User created"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn create_user(
    State(state): State<AppState>,
    AdminRequired(_claims): AdminRequired,
    ValidatedJson(req): ValidatedJson<CreateUserRequest>,
) -> Result<ApiResponse<User>, StatusCode> {
    let hash = hash_password(&req.password);
    let user = state
        .store
        .create_user(&req.username, &hash, req.role)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(ApiResponse::ok(user, StatusCode::CREATED))
}

#[utoipa::path(
    get,
    path="/me",
    responses(
        (status = 200, body = ApiResponse<User>),
        (status = 401, description = "Unauthorized"),
    )
)]
pub async fn get_me(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
) -> Result<ApiResponse<User>, StatusCode> {
    let user = state
        .store
        .get_user_by_id(claims.sub)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(ApiResponse::ok(user, StatusCode::OK))
}

#[utoipa::path(
    post,
    path="/logout",
    responses(
        (status = 200, description = "Logout Successful"),
        (status = 403, description = "Forbidden"),
    )
)]
#[axum::debug_handler]
pub async fn logout(cookies: Cookies) -> ApiResponse<()> {
    let mut cookie = Cookie::from("arbiter_session");
    cookie.make_removal();
    // cookies.add(cookie);
    cookies.remove(cookie);

    ApiResponse::ok((), StatusCode::OK)
}
