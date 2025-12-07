use crate::queries::*;
use crate::requests::*;
use crate::responses::ApiResponse;
use crate::responses::HealthCheckResponse;
use crate::state::AppState;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{
    Json,
    extract::{Query, State},
};
use chrono::Duration;
use chrono::Utc;
use dromio_core::DromioError;
use dromio_core::MisfirePolicy;
use dromio_core::WorkerRecord;
use dromio_core::{JobRun, JobSpec};
use std::str::FromStr;
use uuid::Uuid;

// TODO: Add tenant filter for jons, runs, etc.
// TODO: Tenant users also have a system tag, enforce the related filter on them
// TODO: Perhaps fetch user in the auth filter to keep things safe and reliable

// TODO: Dashboard endpoint? Perhaps reenforce a sane-ish limit for listing runs too

#[utoipa::path(
    post,
    path = "/jobs",
    request_body = CreateJobRequest,
    responses(
        (status = 201, body = ApiResponse<JobSpec>),
        (status = 400, description = "Invalid cron expression"),
        (status = 500, description = "Database error")
    )
)]
#[axum::debug_handler]
pub async fn create_job(
    State(state): State<AppState>,
    Json(req): Json<CreateJobRequest>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    // validate cron
    if let Err(e) = croner::Cron::from_str(req.schedule_cron.as_deref().unwrap_or("* * * * *")) {
        return Ok(ApiResponse::error(
            StatusCode::BAD_REQUEST,
            "invalid_cron",
            format!("{}", e),
        ));
    }

    match state
        .store
        .create_job(
            &req.name,
            req.schedule_cron.clone(),
            req.runner_config.clone(),
            req.max_concurrency.unwrap_or(1),
            req.misfire_policy.unwrap_or(MisfirePolicy::RunImmediately),
        )
        .await
    {
        Ok(job) => Ok(ApiResponse::ok(job, StatusCode::CREATED)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/jobs",
    responses(
        (status = 200, body = ApiResponse<Vec<JobSpec>>)
    )
)]
#[axum::debug_handler]
pub async fn list_jobs(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<JobSpec>>, StatusCode> {
    match state.store.list_jobs().await {
        Ok(jobs) => Ok(ApiResponse::ok(jobs, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

// TODO: Explore way to subscribe to changes with SSE/Websockets for more efficiency(low prio though, only used by admin UI).
// TODO: Perhaps notify from db on changes, or poll on backend for less overall traffic
#[utoipa::path(
    get,
    path = "/runs",
    params(
        ("before" = Option<DateTime<Utc>>, Query, description = "Fetch runs before this timestamp"),
        ("after" = Option<DateTime<Utc>>, Query, description = "Fetch runs after this timestamp"),
        ("by_job_id" = Option<DateTime<Utc>>, Query, description = "Fetch runs related to this job"),
        ("by_worker_id" = Option<DateTime<Utc>>, Query, description = "Fetch runs that run on this worker"),
    ),
    responses(
        (status = 200, body = ApiResponse<Vec<JobRun>>)
    )
)]
#[axum::debug_handler]
pub async fn list_runs(
    State(state): State<AppState>,
    Query(params): Query<ListRunsQuery>,
) -> Result<ApiResponse<Vec<JobRun>>, StatusCode> {
    match state
        .store
        .list_recent_runs(
            params.limit,
            params.before,
            params.after,
            params.by_job_id,
            params.by_worker_id,
        )
        .await
    {
        Ok(runs) => Ok(ApiResponse::ok(runs, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/jobs/{id}",
    responses(
        (status = 200, body = ApiResponse<JobSpec>),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn get_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    match state.store.get_job(job_id).await {
        Ok(job) => Ok(ApiResponse::ok(job, StatusCode::OK)),
        Err(DromioError::NotFound(_)) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("job {} not found", job_id),
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    patch,
    path = "/jobs/{id}",
    request_body = UpdateJobRequest,
    responses(
        (status = 200, body = ApiResponse<JobSpec>),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn update_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
    Json(req): Json<UpdateJobRequest>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    // validate cron if present
    if let Some(Some(cron)) = &req.schedule_cron
        && let Err(e) = croner::Cron::from_str(cron)
    {
        return Ok(ApiResponse::error(
            StatusCode::BAD_REQUEST,
            "invalid_cron",
            format!("{}", e),
        ));
    }

    match state
        .store
        .update_job(
            job_id,
            req.name,
            req.schedule_cron,
            req.runner_config,
            req.max_concurrency,
            req.misfire_policy,
        )
        .await
    {
        Ok(updated) => Ok(ApiResponse::ok(updated, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    delete,
    path = "/jobs/{id}",
    responses(
        (status = 204, description = "Job deleted"),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn delete_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.delete_job(job_id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::NO_CONTENT)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/jobs/{id}/enable",
    responses(
        (status = 200, description = "Job enabled"),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn enable_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.enable_job(job_id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/jobs/{id}/disable",
    responses(
        (status = 200, description = "Job disabled"),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn disable_job(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.disable_job(job_id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

// TODO: Make a different endpoint to reenact a run.
#[utoipa::path(
    post,
    path = "/jobs/{id}/run",
    responses(
        (status = 201, body = ApiResponse<JobRun>),
        (status = 404, description = "Job not found")
    )
)]
#[axum::debug_handler]
pub async fn run_job_now(
    State(state): State<AppState>,
    Path(job_id): Path<Uuid>,
) -> Result<ApiResponse<JobRun>, StatusCode> {
    // TODO: rework to rely on run, and not have arbitrary command as option, but the past or current
    match state.store.create_adhoc_run(job_id).await {
        Ok(run) => Ok(ApiResponse::ok(run, StatusCode::CREATED)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    post,
    path = "/runs/{id}/cancel",
    responses(
        (status = 204, description = "Run cancelled"),
        (status = 404, description = "Run not found")
    )
)]
#[axum::debug_handler]
pub async fn cancel_run(
    State(state): State<AppState>,
    Path(run_id): Path<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.cancel_run(run_id).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::NO_CONTENT)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, body = ApiResponse<HealthCheckResponse>),
    )
)]
#[axum::debug_handler]
pub async fn health_check(
    State(state): State<AppState>,
) -> Result<ApiResponse<HealthCheckResponse>, StatusCode> {
    // TODO: Should it return OK when storage is disconnected?
    let storage = match state.store.health_check().await {
        Ok(_) => "ok",
        Err(e) => {
            eprintln!("Health check failed: {}", e);
            "disconnected"
        }
    };

    let response = HealthCheckResponse {
        status: "ok".into(),
        storage_connected: storage.into(),
    };
    Ok(ApiResponse::ok(response, StatusCode::OK))
}

#[utoipa::path(
    get,
    path = "/workers",
    responses(
        (status = 200, body = ApiResponse<Vec<WorkerRecord>>)
    )
)]
#[axum::debug_handler]
pub async fn list_workers(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<WorkerRecord>>, StatusCode> {
    match state.store.list_workers().await {
        Ok(workers) => {
            // TODO: give workers more consistent IDs
            // Filter out workers not seen for an hour
            let filtered_workers = workers
                .into_iter()
                .filter(|w| w.last_seen > (Utc::now() - Duration::minutes(20)))
                .collect::<Vec<_>>();
            Ok(ApiResponse::ok(filtered_workers, StatusCode::OK))
        }
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[axum::debug_handler]
pub async fn api_not_found() -> impl IntoResponse {
    ApiResponse::<()>::error(StatusCode::NOT_FOUND, "not_found", "resource not found")
}
