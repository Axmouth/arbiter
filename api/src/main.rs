mod queries;
mod requests;
mod responses;
mod routes;
mod state;

use axum::{Extension, extract::ConnectInfo};
use dromio_core::Store;
use dromio_store_pg::PgStore;
use routes::*;
use state::AppState;
use std::sync::Arc;
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, services::ServeDir, trace::TraceLayer,
};
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

#[derive(OpenApi)]
#[openapi()]
struct ApiDoc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // TODO: use config/env; hardcoded for now
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://dromio:dromio@localhost:2345/dromio".into());

    let store = Arc::new(PgStore::new(&database_url).await?);

    run_http_api(store).await?;

    Ok(())
}

// TODO: auth?
pub fn api_router_v1() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(create_job,))
        .routes(routes!(list_jobs))
        .routes(routes!(get_job))
        .routes(routes!(update_job))
        .routes(routes!(delete_job))
        .routes(routes!(enable_job))
        .routes(routes!(disable_job))
        .routes(routes!(run_now))
        .routes(routes!(list_runs))
        .routes(routes!(cancel_run))
        .routes(routes!(list_workers))
        .fallback(api_not_found)
}

pub async fn run_http_api(store: Arc<dyn Store + Send + Sync>) -> anyhow::Result<()> {
    let state = AppState { store };

    // Serve your SPA from "./ui_dist"
    let ui_service = ServeDir::new("ui_dist").fallback(ServeDir::new("ui_dist/index.html"));

    let api_v1 = api_router_v1().layer((
        TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
            let request_id = Uuid::new_v4();

            tracing::info_span!(
                "http_request",
                id = %request_id,
                method = %req.method(),
                uri = %req.uri().path(),
                user_agent = ?req.headers().get("user-agent"),
            )
        }),
        CompressionLayer::new(),
        CorsLayer::very_permissive(),
    ));

    // TODO: use config/env; hardcoded for now
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8080));

    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .layer(Extension(ConnectInfo(addr)))
        .nest("/api/v1", api_v1)
        .routes(routes!(health_check))
        .fallback_service(ui_service)
        .with_state(state)
        .split_for_parts();
    let router = router.merge(SwaggerUi::new("/swagger-ui").url("/apidoc/openapi.json", api));
    let app = router.into_make_service();

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();

    Ok(())
}
