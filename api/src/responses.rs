use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use ts_rs::TS;
use utoipa::ToSchema;

fn is_unit<T: Serialize>(value: &T) -> bool {
    // Unit type () serializes as `null` in serde_json,
    // but we can determine it via JSON value comparison:
    serde_json::to_value(value)
        .map(|v| v.is_null())
        .unwrap_or(false)
}

#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "status")] // adds "status": "ok" or "error"
#[ts(export)]
pub enum ApiResponse<T> {
    #[serde(rename = "ok")]
    Ok {
        #[serde(skip_serializing_if = "is_unit")]
        data: T,
    },

    #[serde(rename = "error")]
    Err {
        error: String,
        message: String,
        code: u16,
    },
}

impl<T: Serialize> IntoResponse for ApiResponse<T> {
    fn into_response(self) -> Response {
        match &self {
            ApiResponse::Ok { data: _ } => (StatusCode::OK, Json(self)).into_response(),
            ApiResponse::Err { code, .. } => {
                let status = StatusCode::from_u16(*code).unwrap_or(StatusCode::BAD_REQUEST);
                (status, Json(self)).into_response()
            }
        }
    }
}

impl<T> ApiResponse<T> {
    pub fn ok(data: T, code: StatusCode) -> Self {
        if !code.is_success() {
            return ApiResponse::Err {
                error: "invalid_status_code".into(),
                message: "Status code for success must be 2xx".into(),
                code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            };
        }
        ApiResponse::Ok { data }
    }

    pub fn error(code: StatusCode, error: &str, msg: impl Into<String>) -> Self {
        if code < StatusCode::BAD_REQUEST {
            return ApiResponse::Err {
                error: "invalid_status_code".into(),
                message: "Status code for error must be 4xx or 5xx".into(),
                code: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            };
        }
        ApiResponse::Err {
            error: error.to_string(),
            message: msg.into(),
            code: code.as_u16(),
        }
    }
}

#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheckResponse {
    pub status: String,
    pub storage_connected: String,
}
