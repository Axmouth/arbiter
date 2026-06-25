use crate::auth::jwt::AuthClaims;
use crate::extractors::ValidatedJson;
use crate::extractors::ValidatedPath;
use crate::extractors::ValidatedQuery;
use crate::queries::*;
use crate::requests::*;
use crate::responses::ApiResponse;
use crate::responses::HealthCheckResponse;
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use chrono::Duration;
use chrono::Utc;
use arbiter_core::ArbiterError;
use arbiter_core::MisfirePolicy;
use arbiter_core::Setting;
use arbiter_core::WorkerRecord;
use arbiter_core::{JobRun, JobSpec};
use std::collections::HashMap;
use std::str::FromStr;
use uuid::Uuid;

// TODO: Dashboard endpoint? Perhaps reenforce a sane-ish limit for listing runs too

/// Returns an error response if the job is outside the caller's tenant scope (or missing),
/// so a tenant caller cannot read or mutate another tenant's job. `None` for a system caller.
async fn job_scope_error(
    state: &AppState,
    job_id: Uuid,
    scope: Option<Uuid>,
) -> Option<(StatusCode, &'static str, String)> {
    match state.store.get_job(job_id, scope).await {
        Ok(_) => None,
        Err(ArbiterError::NotFound(_)) => {
            Some((StatusCode::NOT_FOUND, "not_found", format!("job {job_id} not found")))
        }
        Err(e) => Some((StatusCode::INTERNAL_SERVER_ERROR, "db_error", e.to_string())),
    }
}

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
    AuthClaims(claims): AuthClaims,
    ValidatedJson(req): ValidatedJson<CreateJobRequest>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    // validate cron
    if let Err(e) = croner::Cron::from_str(req.schedule_cron.as_deref().unwrap_or("* * * * *")) {
        return Ok(ApiResponse::error(
            StatusCode::BAD_REQUEST,
            "invalid_cron",
            format!("{}", e),
        ));
    }

    let job = match state
        .store
        .create_job(
            claims.create_tenant(),
            &req.name,
            req.schedule_cron.clone(),
            req.runner_config.clone(),
            req.max_concurrency.unwrap_or(1),
            req.misfire_policy.unwrap_or(MisfirePolicy::RunImmediately),
            req.retry.unwrap_or_default(),
        )
        .await
    {
        Ok(job) => job,
        Err(e) => {
            return Ok(ApiResponse::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "db_error",
                e.to_string(),
            ));
        }
    };

    if let Some(env) = req.env
        && let Err(e) = state.store.set_job_env(job.id, env).await
    {
        return Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        ));
    }

    Ok(ApiResponse::ok(job, StatusCode::CREATED))
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
    AuthClaims(claims): AuthClaims,
) -> Result<ApiResponse<Vec<JobSpec>>, StatusCode> {
    match state.store.list_jobs(claims.scope()).await {
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
    AuthClaims(claims): AuthClaims,
    ValidatedQuery(params): ValidatedQuery<ListRunsQuery>,
) -> Result<ApiResponse<Vec<JobRun>>, StatusCode> {
    match state
        .store
        .list_recent_runs(
            params.limit,
            params.before,
            params.after,
            params.by_job_id,
            params.by_worker_id,
            claims.scope(),
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    match state.store.get_job(job_id, claims.scope()).await {
        Ok(job) => Ok(ApiResponse::ok(job, StatusCode::OK)),
        Err(ArbiterError::NotFound(_)) => Ok(ApiResponse::error(
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
    get,
    path = "/jobs/{id}/env",
    responses(
        (status = 200, body = ApiResponse<HashMap<String, String>>)
    )
)]
#[axum::debug_handler]
pub async fn get_job_env(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<HashMap<String, String>>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
    match state.store.get_job_env(job_id).await {
        Ok(env) => Ok(ApiResponse::ok(env, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

#[utoipa::path(
    put,
    path = "/jobs/{id}/env",
    request_body = SetJobEnvRequest,
    responses(
        (status = 200, description = "Environment variables replaced")
    )
)]
#[axum::debug_handler]
pub async fn set_job_env(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
    ValidatedJson(req): ValidatedJson<SetJobEnvRequest>,
) -> Result<ApiResponse<()>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
    match state.store.set_job_env(job_id, req.env).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::OK)),
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
    ValidatedJson(req): ValidatedJson<UpdateJobRequest>,
) -> Result<ApiResponse<JobSpec>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }

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

    if let Some(env) = req.env
        && let Err(e) = state.store.set_job_env(job_id, env).await
    {
        return Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
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
            req.retry,
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<()>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
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
    AuthClaims(claims): AuthClaims,
    ValidatedPath(job_id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<JobRun>, StatusCode> {
    if let Some((sc, code, msg)) = job_scope_error(&state, job_id, claims.scope()).await {
        return Ok(ApiResponse::error(sc, code, msg));
    }
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

// TODO: consider gating this to admin/operator roles once role checks are in place.
#[utoipa::path(
    post,
    path = "/runs/prune",
    params(
        ("older_than_days" = Option<u32>, Query, description = "Delete terminal runs older than this many days (default 30)")
    ),
    responses(
        (status = 200, body = ApiResponse<u64>, description = "Number of runs pruned")
    )
)]
#[axum::debug_handler]
pub async fn prune_runs(
    State(state): State<AppState>,
    ValidatedQuery(params): ValidatedQuery<PruneRunsQuery>,
) -> Result<ApiResponse<u64>, StatusCode> {
    let cutoff = Utc::now() - Duration::days(params.older_than_days.unwrap_or(30) as i64);
    match state.store.prune_runs(cutoff).await {
        Ok(n) => Ok(ApiResponse::ok(n, StatusCode::OK)),
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
    ValidatedPath(run_id): ValidatedPath<Uuid>,
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
    path = "/settings",
    responses(
        (status = 200, body = ApiResponse<Vec<Setting>>)
    )
)]
#[axum::debug_handler]
pub async fn list_settings(
    State(state): State<AppState>,
) -> Result<ApiResponse<Vec<Setting>>, StatusCode> {
    match state.store.list_settings().await {
        Ok(settings) => Ok(ApiResponse::ok(settings, StatusCode::OK)),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

// TODO: validate/whitelist setting keys; gate to admin/operator roles once available.
#[utoipa::path(
    put,
    path = "/settings/{key}",
    request_body = SetSettingRequest,
    responses(
        (status = 200, description = "Setting updated")
    )
)]
#[axum::debug_handler]
pub async fn set_setting(
    State(state): State<AppState>,
    ValidatedPath(key): ValidatedPath<String>,
    ValidatedJson(req): ValidatedJson<SetSettingRequest>,
) -> Result<ApiResponse<()>, StatusCode> {
    match state.store.set_setting(&key, &req.value).await {
        Ok(()) => Ok(ApiResponse::ok((), StatusCode::OK)),
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

#[utoipa::path(
    get,
    path = "/runs/{id}",
    responses(
        (status = 200, body = ApiResponse<JobRun>),
        (status = 404, description = "Run not found or out of scope")
    )
)]
#[axum::debug_handler]
pub async fn get_run(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> Result<ApiResponse<JobRun>, StatusCode> {
    match state.store.get_run(id, claims.scope()).await {
        Ok(Some(run)) => Ok(ApiResponse::ok(run, StatusCode::OK)),
        Ok(None) => Ok(ApiResponse::error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("run {id} not found"),
        )),
        Err(e) => Ok(ApiResponse::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "db_error",
            e.to_string(),
        )),
    }
}

/// Server-Sent Events stream of a single run's state and captured output as it executes,
/// pushing a fresh `JobRun` snapshot on each runs-change notification and closing once the
/// run reaches a terminal state. Authenticated by the session cookie; tenant-scoped.
pub async fn run_stream(
    State(state): State<AppState>,
    AuthClaims(claims): AuthClaims,
    ValidatedPath(id): ValidatedPath<Uuid>,
) -> axum::response::sse::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    let scope = claims.scope();
    let wake_store = state.store.clone();
    let snap_store = state.store.clone();
    crate::sse::snapshot_stream(
        std::time::Duration::from_secs(15),
        move || {
            let store = wake_store.clone();
            Box::pin(async move {
                store.await_runs_change().await;
            })
        },
        move || {
            let store = snap_store.clone();
            Box::pin(async move {
                match store.get_run(id, scope).await {
                    Ok(Some(run)) => {
                        let terminal = run.state.is_terminal();
                        match axum::response::sse::Event::default().json_data(run) {
                            Ok(ev) => Some((ev, terminal)),
                            Err(_) => None,
                        }
                    }
                    // Missing or out of scope: nothing to stream, close.
                    _ => None,
                }
            })
        },
    )
}

/// Server-Sent Events stream that pings whenever runs change (the `arbiter_runs` notify
/// channel) so the dashboard refetches on change instead of polling on a fixed timer.
/// Authenticated by the session cookie the browser `EventSource` sends. The ping carries no
/// data, so it needs no tenant scope; the client's refetch is already scoped.
pub async fn runs_stream(
    State(state): State<AppState>,
    AuthClaims(_claims): AuthClaims,
) -> axum::response::sse::Sse<
    impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    let store = state.store.clone();
    crate::sse::change_stream(std::time::Duration::from_secs(20), move || {
        let store = store.clone();
        Box::pin(async move {
            store.await_runs_change().await;
        })
    })
}

#[axum::debug_handler]
pub async fn api_not_found() -> impl IntoResponse {
    ApiResponse::<()>::error(StatusCode::NOT_FOUND, "not_found", "resource not found")
}
