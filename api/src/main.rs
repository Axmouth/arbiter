mod auth;
mod extractors;
mod queries;
mod requests;
mod responses;
mod routes;
mod state;
mod users;

use axum::extract::ConnectInfo;
use axum::extract::rejection::JsonRejection;
use axum::http::{StatusCode, header};
use axum::middleware;
use axum::routing::get_service;
use arbiter_config::WebConfig;
use arbiter_core::Store;
use arbiter_store_pg::PgStore;
use routes::*;
use state::AppState;
use tower_http::services::ServeFile;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_cookies::CookieManagerLayer;
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, services::ServeDir, trace::TraceLayer,
};
use users::routes::*;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

use crate::auth::jwt::JwtKeys;
use crate::auth::middleware::require_auth;
use crate::responses::ApiResponse;
use crate::users::seed_admin;


impl From<JsonRejection> for ApiResponse<()> {
    fn from(rejection: JsonRejection) -> Self {
        Self::error(StatusCode::BAD_REQUEST, "Invalid JSON", format!("Invalid JSON: {}", rejection))
    }
}

#[derive(OpenApi)]
#[openapi()]
struct ApiDoc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cfg = WebConfig::try_load()?;

    let store = Arc::new(PgStore::new(&cfg.database.url).await?);

    seed_admin(&*store, &cfg.admin).await?;

    run_http_api(store, &cfg).await?;

    Ok(())
}

// TODO: Node management endpoints (Change per node config through admin ui, make a node preferred for a role, add/remove? Should there be a protocol to config a node to join the cluster?)
pub fn api_router_v1(keys: JwtKeys) -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_job,))
        .routes(routes!(list_jobs))
        .routes(routes!(get_job))
        .routes(routes!(update_job))
        .routes(routes!(delete_job))
        .routes(routes!(enable_job))
        .routes(routes!(disable_job))
        .routes(routes!(run_job_now))
        .routes(routes!(list_runs))
        .routes(routes!(cancel_run))
        .routes(routes!(list_workers))
        .route_layer(middleware::from_fn_with_state(keys.clone(), require_auth))
        .fallback(api_not_found)
}

pub fn auth_router(keys: JwtKeys) -> OpenApiRouter<AppState> {
    let gated_router = OpenApiRouter::new()
        .routes(routes!(logout,))
        .routes(routes!(get_me,))
        .routes(routes!(list_users,))
        .routes(routes!(get_user,))
        .routes(routes!(create_user,))
        .routes(routes!(update_user,))
        .routes(routes!(delete_user,))
        .route_layer(middleware::from_fn_with_state(keys.clone(), require_auth));
    OpenApiRouter::new()
        .routes(routes!(login,))
        .merge(gated_router)
        .fallback(api_not_found)
}

pub async fn run_http_api(
    store: Arc<dyn Store + Send + Sync>,
    cfg: &WebConfig,
) -> anyhow::Result<()> {
    let jwt_secret = &cfg.api.jwt_secret;
    let jwt_keys = JwtKeys::from_secret(jwt_secret);
    let state = AppState {
        store,
        jwt_keys: jwt_keys.clone(),
    };

    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(|req: &axum::http::Request<_>| {
            tracing::info_span!(
                "http_request",
                request_id = %Uuid::new_v4(),
                method = %req.method(),
                uri = %req.uri().path(),
                user_agent = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()),
                remote_ip = extract_client_ip(req),
            )
        })
        .on_request(|_request: &axum::http::Request<_>, _span: &tracing::Span| {
            tracing::info!("request started");
        })
        .on_response(
            |response: &axum::http::Response<_>,
             latency: std::time::Duration,
             span: &tracing::Span| {
                tracing::info!(
                    parent: span,
                    status = %response.status(),
                    latency = ?latency,
                    "request completed"
                );
            },
        );

    let api_v1 = api_router_v1(jwt_keys.clone()).layer(trace_layer.clone());

    let auth_api = auth_router(jwt_keys).layer(trace_layer);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], cfg.api.port));

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest("/api/v1", api_v1)
        .nest("/api", auth_api)
        .routes(routes!(health_check))
        .layer(CookieManagerLayer::new())
        .with_state(state)
        .split_for_parts();
    let router = router
        .merge(SwaggerUi::new("/swagger-ui").url("/apidoc/openapi.json", api))
        .layer((CompressionLayer::new(), CorsLayer::permissive()));

    // Serve SPA from "./ui_dist"
    let static_dir = ServeDir::new("ui_dist")
        .fallback(ServeFile::new("ui_dist/index.html"));


    // 1) Serve static files normally
    let router = router
        .fallback_service(
            get_service(static_dir).handle_error(|_| async {
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        )
        .layer((CompressionLayer::new(), CorsLayer::permissive()));
    let app = router.into_make_service_with_connect_info::<SocketAddr>();

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}

fn extract_client_ip<B>(req: &axum::http::Request<B>) -> Option<String> {
    // RFC 7239 Forwarded: for=1.2.3.4
    if let Some(forwarded) = req.headers().get("forwarded")
        && let Ok(forwarded) = forwarded.to_str()
    {
        // crude parse: look for "for="
        if let Some(for_part) = forwarded
            .split(';')
            .find(|s| s.trim_start().starts_with("for="))
        {
            return Some(
                for_part
                    .trim()
                    .trim_start_matches("for=")
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }

    // X-Forwarded-For: client, proxy1, proxy2,...
    if let Some(xff) = req.headers().get("x-forwarded-for")
        && let Ok(xff) = xff.to_str()
    {
        return Some(xff.split(',').next().unwrap().trim().to_string());
    }

    // last resort: TCP peer
    req.extensions()
        .get::<ConnectInfo<std::net::SocketAddr>>()
        .map(|ci| ci.0.ip().to_string())
}
