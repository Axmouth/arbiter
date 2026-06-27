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

/// Secret metadata. Deliberately carries no ciphertext or plaintext: secret values are
/// write-only and never leave the server.
#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SecretMetaResponse {
    pub id: uuid::Uuid,
    pub name: String,
    pub kek_version: u32,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<arbiter_core::SecretMeta> for SecretMetaResponse {
    fn from(m: arbiter_core::SecretMeta) -> Self {
        Self {
            id: m.id,
            name: m.name,
            kek_version: m.kek_version,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// A page of a run's log chunks plus the overall size, so the client can decide whether to
/// page further (load earlier or follow newer).
#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunLogPage {
    pub chunks: Vec<arbiter_core::LogChunk>,
    pub size: arbiter_core::LogSize,
}

/// A snapshot of KEK rotation progress. `phase` is one of `idle`, `distributing`,
/// `rewrapping`, `done`. On a single node a rotation returns `done` immediately; on a
/// cluster it may report `distributing` while waiting for other nodes to ack the new key.
#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RotateKekResponse {
    pub phase: String,
    pub active_version: Option<u32>,
    pub target_version: Option<u32>,
    pub nodes_acked: u32,
    pub nodes_total: u32,
    pub secrets_rewrapped: u32,
    pub secrets_total: u32,
}

impl From<arbiter_core::RotationStatus> for RotateKekResponse {
    fn from(s: arbiter_core::RotationStatus) -> Self {
        let phase = match s.phase {
            arbiter_core::RotationPhase::Idle => "idle",
            arbiter_core::RotationPhase::Distributing => "distributing",
            arbiter_core::RotationPhase::Rewrapping => "rewrapping",
            arbiter_core::RotationPhase::Done => "done",
        };
        Self {
            phase: phase.to_string(),
            active_version: s.active_version,
            target_version: s.target_version,
            nodes_acked: s.nodes_acked,
            nodes_total: s.nodes_total,
            secrets_rewrapped: s.secrets_rewrapped,
            secrets_total: s.secrets_total,
        }
    }
}

/// A registered node's key and its KEK-approval status. `publicKey` is hex (it is public,
/// safe to show, and lets an admin verify the fingerprint out of band before approving).
#[derive(Serialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NodeKeyResponse {
    pub node_id: uuid::Uuid,
    pub key_version: u32,
    pub status: String,
    pub public_key: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub approved_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<arbiter_core::StoredNodeKey> for NodeKeyResponse {
    fn from(k: arbiter_core::StoredNodeKey) -> Self {
        let public_key = k.public_key.iter().map(|b| format!("{b:02x}")).collect();
        Self {
            node_id: k.node_id,
            key_version: k.key_version,
            status: k.status,
            public_key,
            created_at: k.created_at,
            approved_at: k.approved_at,
        }
    }
}
