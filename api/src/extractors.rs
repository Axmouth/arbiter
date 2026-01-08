use axum::{
    Json, RequestExt, extract::{FromRequest, FromRequestParts, Path, Query}, http::{StatusCode, request::Parts}
};
use serde::de::DeserializeOwned;

use crate::responses::ApiResponse;

pub struct ValidatedJson<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedJson<T>
where
    T: DeserializeOwned + Send + 'static,
    S: Send + Sync,
{
    type Rejection = ApiResponse<()>;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match req.extract::<Json<T>, _>().await {
            Ok(Json(value)) => Ok(ValidatedJson(value)),
            Err(err) => Err(ApiResponse::error(
                StatusCode::BAD_REQUEST,
                "Invalid JSON",
                err.to_string(),
            )),
        }
    }
}

pub struct ValidatedQuery<T>(pub T);

impl<S, T> FromRequest<S> for ValidatedQuery<T>
where
    T: DeserializeOwned + Send + 'static,
    S: Send + Sync,
{
    type Rejection = ApiResponse<()>;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match req.extract::<Query<T>, _>().await {
            Ok(Query(value)) => Ok(ValidatedQuery(value)),
            Err(err) => Err(ApiResponse::error(
                StatusCode::BAD_REQUEST,
                "Invalid Query",
                err.to_string(),
            )),
        }
    }
}

pub struct ValidatedPath<T>(pub T);

impl<S, T> FromRequestParts<S> for ValidatedPath<T>
where
    T: DeserializeOwned + Send + 'static,
    S: Send + Sync,
{
    type Rejection = ApiResponse<()>;

    async fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        match Path::<T>::from_request_parts(parts, _state).await {
            Ok(Path(value)) => Ok(ValidatedPath(value)),
            Err(err) => Err(ApiResponse::error(
                StatusCode::BAD_REQUEST,
                "Invalid Path",
                err.to_string(),
            )),
        }
    }
}
