use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use tower_cookies::Cookies;

use crate::{
    auth::jwt::{JwtKeys, decode_jwt},
    responses::ApiResponse,
};

fn create_auth_error() -> ApiResponse<()> {
    ApiResponse::error(
        StatusCode::UNAUTHORIZED,
        "Unauthorized",
        "This endpoint requires authorization",
    )
}

pub async fn require_auth(
    cookies: Cookies,
    State(keys): State<JwtKeys>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiResponse<()>> {
    let Some(cookie) = cookies.get("dromio_session") else {
        return Err(create_auth_error());
    };

    let token = cookie.value();

    let claims = decode_jwt(token, &keys).map_err(|_| create_auth_error())?;

    // Attach user claims into request extensions
    req.extensions_mut().insert(claims);

    Ok(next.run(req).await)
}
