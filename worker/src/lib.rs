use chrono::{DateTime, Duration, Utc};
use arbiter_core::{
    ArbiterError, ExecutableConfigSnapshotMeta, JobRun, JobRunState, ResultStatus, Result, RunOutcome,
    Store, WorkerConfig, WorkerRecord, next_retry_delay, snooze,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::process::Command;
use uuid::Uuid;

/// Decrements the in-flight task counter when a spawned run finishes (any path).
struct RunGuard(Arc<AtomicU32>);
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

// TODO: algo to determine job's "work units" over time? And worker capacity?
pub async fn run_worker_loop(store: Arc<dyn Store + Send + Sync>, cfg: WorkerConfig) -> ! {
    let mut last_heartbeat: Option<DateTime<Utc>> = None;
    let mut last_prune: Option<DateTime<Utc>> = None;
    // In-flight run tasks, so the worker honors its capacity instead of over-spawning.
    let running = Arc::new(AtomicU32::new(0));

    loop {
        let now = Utc::now();

        // Heartbeat periodically
        let needs_heartbeat = last_heartbeat
            .map(|t| (now - t).num_milliseconds() >= cfg.heartbeat_interval_ms as i64)
            .unwrap_or(true);

        if needs_heartbeat {
            let rec = WorkerRecord {
                id: cfg.worker_id,
                display_name: cfg.display_name.clone(),
                hostname: cfg.hostname.clone(),
                last_seen: now,
                capacity: cfg.capacity,
                restart_count: cfg.restart_count,
                version: env!("CARGO_PKG_VERSION").to_string(),
            };

            if let Err(e) = store.heartbeat(&rec).await {
                tracing::error!("{}: heartbeat failed: {e:?}", cfg.worker_id);
            } else {
                last_heartbeat = Some(now);
            }

            // TODO: later, only do this on leader/reaper worker
            // Reclaim dead workers' jobs as part of "maintenance"
            if let Err(e) = store.reclaim_dead_workers_jobs(cfg.dead_after_secs).await {
                tracing::error!("{}: reclaim_dead_workers_jobs failed: {e:?}", cfg.worker_id);
            }

            // TODO: Prune older runs occasionally? different soft and hard delete windows.
            // TODO: Soft to make smaller index queries, hard to keep storage in check
        }

        // Retention: the leader prunes old terminal runs on its own interval.
        // Runtime settings override the static config defaults (read live).
        let retention_days = match store.get_setting("retention.run_retention_days").await {
            Ok(Some(v)) => v.parse::<u64>().unwrap_or(cfg.run_retention_secs / 86_400),
            _ => cfg.run_retention_secs / 86_400,
        };
        let retention_secs = retention_days * 86_400;
        if retention_secs > 0 {
            let prune_interval = match store.get_setting("retention.prune_interval_secs").await {
                Ok(Some(v)) => v.parse::<u64>().unwrap_or(cfg.prune_interval_secs),
                _ => cfg.prune_interval_secs,
            };
            let due = last_prune
                .map(|t| (now - t).num_seconds() as u64 >= prune_interval)
                .unwrap_or(true);
            if due {
                match store.am_i_leader().await {
                    Ok(true) => {
                        let cutoff = now - Duration::seconds(retention_secs as i64);
                        match store.prune_runs(cutoff).await {
                            Ok(n) if n > 0 => {
                                tracing::info!("{}: pruned {n} runs past retention", cfg.worker_id)
                            }
                            Ok(_) => {}
                            Err(e) => {
                                tracing::error!("{}: prune_runs failed: {e:?}", cfg.worker_id)
                            }
                        }
                        last_prune = Some(now);
                    }
                    // Not leader: back off a full interval so we do not spam am_i_leader.
                    Ok(false) => last_prune = Some(now),
                    Err(e) => tracing::error!("{}: am_i_leader failed: {e:?}", cfg.worker_id),
                }
            }
        }

        // Run a worker tick: claim + spawn jobs
        if let Err(e) = worker_tick(store.clone(), &cfg, &running).await {
            tracing::error!("{}: worker_tick error: {e:?}", cfg.worker_id);
        }

        snooze(std::time::Duration::from_millis(cfg.tick_interval_ms), 30).await;
    }
}

pub async fn worker_tick(
    store: Arc<dyn Store + Sync + Send>,
    cfg: &WorkerConfig,
    running: &Arc<AtomicU32>,
) -> Result<()> {
    let available = cfg.capacity.saturating_sub(running.load(Ordering::Relaxed));
    if available == 0 {
        snooze(std::time::Duration::from_millis(200), 30).await;
        return Ok(());
    }

    let runs = store.claim_job_runs(cfg.worker_id, available).await?;
    let runs_num = runs.len();

    let wid = cfg.worker_id;
    for run in runs {
        running.fetch_add(1, Ordering::Relaxed);
        spawn_run_task(store.clone(), wid, run, running.clone());
    }

    if runs_num > 0 {
        tracing::info!("{wid}: claimed and spawned {runs_num} job runs");
    }

    Ok(())
}

fn spawn_run_task(
    store: Arc<dyn Store + Sync + Send>,
    worker_id: Uuid,
    run: JobRun,
    running: Arc<AtomicU32>,
) {
    tokio::spawn(async move {
        let _guard = RunGuard(running);
        tracing::info!(
            "{worker_id}: starting job run {}, for Job {}, Scheduled for {}",
            run.id,
            run.job_id,
            run.scheduled_for
        );

        let snapshot = match run.snapshot {
            Some(snap) => snap,
            None => {
                return store
                    .finalize_run(
                        run.id,
                        JobRunState::Failed,
                        RunOutcome {
                            status: Some(ResultStatus::Failed),
                            error: Some("No config snapshot found, aborting run".to_string()),
                            error_media_type: Some("text/plain".to_string()),
                            ..Default::default()
                        },
                    )
                    .await;
            }
        };

        let run_output: Result<RunOutcome> = match snapshot.meta {
            ExecutableConfigSnapshotMeta::Shell {
                command,
                working_dir,
                env,
            } => {
                let mut cmd = build_shell_command(&command);
                if let Some(dir) = &working_dir {
                    cmd.current_dir(dir);
                }
                run_subprocess(worker_id, run.id, cmd, &env, None)
                    .await
                    .map(process_outcome)
            }
            ExecutableConfigSnapshotMeta::Python {
                module,
                class_name,
                timeout_sec,
                env,
            } => {
                execute_runtime(
                    worker_id,
                    run.id,
                    Lang::Python,
                    &module,
                    &class_name,
                    &env,
                    timeout_sec,
                )
                .await
            }
            ExecutableConfigSnapshotMeta::Node {
                module,
                function_name,
                timeout_sec,
                env,
            } => {
                execute_runtime(
                    worker_id,
                    run.id,
                    Lang::Node,
                    &module,
                    &function_name,
                    &env,
                    timeout_sec,
                )
                .await
            }
            ExecutableConfigSnapshotMeta::Http {
                method,
                url,
                headers,
                body,
                timeout_sec,
            } => {
                execute_http_request(
                    worker_id,
                    run.id,
                    &method,
                    &url,
                    &headers,
                    body.as_deref(),
                    timeout_sec,
                )
                .await
            }
            // pgsql/mysql carry secrets and are not executed yet (see FOLLOWUPS §13).
            _ => {
                return Err(ArbiterError::ExecutionError(format!(
                    "Not Implemented {} yet",
                    snapshot.meta.type_of_str()
                )));
            }
        };

        // Any execution error (incl. unimplemented runners) becomes a failed outcome,
        // so the run is finalized rather than left stuck in `running`.
        let outcome = run_output.unwrap_or_else(|e| RunOutcome {
            status: Some(ResultStatus::Failed),
            error: Some(e.to_string()),
            error_media_type: Some("text/plain".to_string()),
            ..Default::default()
        });

        match outcome.status.unwrap_or(ResultStatus::Failed) {
            ResultStatus::Success => {
                store
                    .finalize_run(run.id, JobRunState::Succeeded, outcome)
                    .await
            }
            ResultStatus::Failed => {
                store.finalize_run(run.id, JobRunState::Failed, outcome).await
            }
            ResultStatus::Retryable => {
                // Requeue with backoff while attempts remain; otherwise it fails.
                match store.get_job(run.job_id).await {
                    Ok(job) if run.attempt < job.max_attempts => {
                        let delay = next_retry_delay(
                            job.backoff_strategy,
                            job.backoff_base_secs,
                            job.backoff_cap_secs,
                            run.attempt,
                        );
                        let next = Utc::now() + delay;
                        tracing::info!(
                            "{worker_id}: run {} retryable (attempt {}/{}), retrying in {}s",
                            run.id,
                            run.attempt,
                            job.max_attempts,
                            delay.num_seconds()
                        );
                        store
                            .reschedule_for_retry(run.id, run.attempt + 1, next, outcome)
                            .await
                    }
                    Ok(_) => store.finalize_run(run.id, JobRunState::Failed, outcome).await,
                    Err(e) => {
                        tracing::error!("{worker_id}: get_job for retry failed: {e:?}");
                        store.finalize_run(run.id, JobRunState::Failed, outcome).await
                    }
                }
            }
        }
    });
}

/// The raw streams + exit code from a spawned process (the universal denominator).
pub struct CommandRunOutput {
    exit_code: i32,
    stdout: Option<String>,
    stderr: Option<String>,
}

/// Classify a process exit code: 0 = success, 75 (EX_TEMPFAIL) = retryable, else failed.
fn status_from_exit(code: i32) -> ResultStatus {
    match code {
        0 => ResultStatus::Success,
        75 => ResultStatus::Retryable,
        _ => ResultStatus::Failed,
    }
}

/// Map a plain process result (shell, or a runtime that wrote no result file) onto a
/// `RunOutcome`: streams stay text, no typed result/error, status from the exit code.
fn process_outcome(c: CommandRunOutput) -> RunOutcome {
    RunOutcome {
        status: Some(status_from_exit(c.exit_code)),
        exit_code: Some(c.exit_code),
        stdout: c.stdout,
        stderr: c.stderr,
        ..Default::default()
    }
}

/// Build the command for a shell runner (platform shell + the script string).
fn build_shell_command(command: &str) -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(command);
        cmd
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

/// Vendored, dependency-free language runtimes (Layer B). The worker writes one
/// into the run's temp dir and invokes it; it imports the user's module, runs the
/// entrypoint, and writes a structured result document. See RUNNER_RESULT_PROTOCOL.md.
const PYTHON_RUNTIME: &str = include_str!("../runtimes/arbiter_runtime.py");
const NODE_RUNTIME: &str = include_str!("../runtimes/arbiter_runtime.js");

const PROTOCOL_VERSION: &str = "1";

enum Lang {
    Python,
    Node,
}

/// The result document a runtime writes to `ARBITER_RESULT_FILE`.
#[derive(Deserialize)]
struct RuntimeResult {
    #[serde(default)]
    status: String,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RuntimeError>,
}

#[derive(Deserialize)]
struct RuntimeError {
    #[serde(rename = "type", default)]
    type_: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    stack: Option<Vec<String>>,
}

/// Ensure the vendored runtime file exists on disk and return its path. The file is
/// content-addressed (name carries a hash of the source), so it is written once and
/// reused across runs, and a runtime edit auto-invalidates the old file. The write
/// is atomic (temp + rename) so concurrent first-writers cannot read a partial file.
fn ensure_runtime_file(stem: &str, ext: &str, source: &str) -> Result<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    let hash = hasher.finish();

    let dir = std::env::temp_dir().join("arbiter-runtime");
    std::fs::create_dir_all(&dir).map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
    let path = dir.join(format!("{stem}_{hash:016x}.{ext}"));
    if path.exists() {
        return Ok(path);
    }

    let mut tmp = tempfile::Builder::new()
        .prefix(".tmp-")
        .tempfile_in(&dir)
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
    std::io::Write::write_all(&mut tmp, source.as_bytes())
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
    match tmp.persist(&path) {
        Ok(_) => Ok(path),
        // A concurrent writer won the race; the file is present and valid.
        Err(_) if path.exists() => Ok(path),
        Err(e) => Err(ArbiterError::ExecutionError(e.to_string())),
    }
}

/// Run a Python/Node job through the injected runtime: ensure the (reused) runtime
/// file exists, hand the child the handshake on argv (module/entry/result-file/...),
/// run it via the shared `run_subprocess` with the job's env untouched, then resolve
/// the result file. Falls back to the raw process outcome (exit code + captured
/// streams) if no valid result file is written. The result file is a `tempfile`
/// whose `TempPath` deletes it on drop (cleanup owned here, upstairs).
async fn execute_runtime(
    worker_id: Uuid,
    run_id: Uuid,
    lang: Lang,
    module: &str,
    entry: &str,
    env: &HashMap<String, String>,
    timeout_sec: Option<u32>,
) -> Result<RunOutcome> {
    let (stem, ext, source, program) = match lang {
        Lang::Python => ("arbiter_runtime", "py", PYTHON_RUNTIME, "python3"),
        Lang::Node => ("arbiter_runtime", "js", NODE_RUNTIME, "node"),
    };

    let runtime_path = ensure_runtime_file(stem, ext, source)?;

    // Unique result file; our handle is closed (into_temp_path) so the child can
    // write it (also on Windows), and it is deleted when `result_path` drops.
    let result_path = tempfile::Builder::new()
        .prefix("arbiter-result-")
        .suffix(".json")
        .tempfile()
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?
        .into_temp_path();

    let mut cmd = Command::new(program);
    cmd.arg(&runtime_path)
        .arg("--module")
        .arg(module)
        .arg("--entry")
        .arg(entry)
        .arg("--result-file")
        .arg(&*result_path)
        .arg("--run-id")
        .arg(run_id.to_string())
        .arg("--transport")
        .arg("file")
        .arg("--protocol")
        .arg(PROTOCOL_VERSION);

    // Env carries only the job's own variables (PYTHONPATH/NODE_PATH/...); the
    // arbiter handshake travels on argv, so we never pollute the user's env.
    let raw = run_subprocess(worker_id, run_id, cmd, env, timeout_sec).await?;

    match tokio::fs::read(&result_path).await {
        Ok(bytes) => match serde_json::from_slice::<RuntimeResult>(&bytes) {
            Ok(res) => Ok(synthesize_runtime_result(res, raw)),
            Err(e) => Ok(RunOutcome {
                status: Some(ResultStatus::Failed),
                exit_code: Some(raw.exit_code),
                stdout: raw.stdout,
                stderr: raw.stderr,
                error: Some(format!("invalid runtime result document: {e}")),
                error_media_type: Some("text/plain".to_string()),
                ..Default::default()
            }),
        },
        // No result file: the child died before reporting -> fall back to the plain
        // process outcome (carries timeout/transport errors and captured streams).
        Err(_) => Ok(process_outcome(raw)),
    }
}

/// Map a runtime result document onto a `RunOutcome`. The process streams stay text
/// (stdout/stderr); the return value becomes the typed `result` (a bare string is
/// `text/plain`, anything else `application/json`); a structured error becomes the
/// typed `error` (`application/json`).
fn synthesize_runtime_result(res: RuntimeResult, raw: CommandRunOutput) -> RunOutcome {
    let status = res
        .status
        .parse::<ResultStatus>()
        .unwrap_or(ResultStatus::Failed);

    let (result, result_media_type) = match res.output {
        Some(serde_json::Value::String(s)) => (Some(s), Some("text/plain".to_string())),
        Some(other) => (Some(other.to_string()), Some("application/json".to_string())),
        None => (None, None),
    };

    let (error, error_media_type) = match res.error {
        Some(e) => {
            let doc = serde_json::json!({
                "type": e.type_,
                "message": e.message,
                "stack": e.stack,
            });
            (Some(doc.to_string()), Some("application/json".to_string()))
        }
        None => (None, None),
    };

    RunOutcome {
        status: Some(status),
        exit_code: Some(raw.exit_code),
        stdout: raw.stdout,
        stderr: raw.stderr,
        result,
        result_media_type,
        error,
        error_media_type,
    }
}

// TODO: run configs and different runners
// TODO: such as ssh config, pg/mysql etc, docker, kubernetes, etc.
// TODO: config setups are separate and can be shared between jobs, also tested on the UI side, as test connecting to db for a db job config, etc.
// TODO: This can conveniently cover real world use cases like running DB backups, calling webhooks, running remote jobs etc.
// TODO: Optionally "preload" runners/setup before running jobs a tiny bit before the actual run time, e.g. docker images, ssh connections, db connections, authentication steps, etc.
// TODO: Potentially start by grabbing jobs a few seconds earlier than scheduled time to allow preloading/setup time. And wait until scheduled time to actually run the job.
/// Spawn a prepared subprocess command, apply the job env, optionally enforce a
/// timeout, and capture stdout/stderr into a `CommandRunOutput`. Shared by the
/// shell, python, and node runners so they map onto the same success/failure path.
async fn run_subprocess(
    worker_id: Uuid,
    run_id: Uuid,
    mut cmd: Command,
    env: &HashMap<String, String>,
    timeout_sec: Option<u32>,
) -> Result<CommandRunOutput> {
    for (k, v) in env {
        cmd.env(k, v);
    }

    // kill_on_drop so a timed-out child is reaped when we drop the wait future.
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;

    // wait_with_output drains both pipes concurrently, so it cannot deadlock on a
    // child that fills its stderr while we wait on stdout.
    let collect = child.wait_with_output();
    let output = match timeout_sec {
        Some(secs) if secs > 0 => {
            match tokio::time::timeout(std::time::Duration::from_secs(secs as u64), collect).await {
                Ok(res) => res.map_err(|e| ArbiterError::ExecutionError(e.to_string()))?,
                Err(_) => {
                    return Ok(CommandRunOutput {
                        exit_code: -1,
                        stdout: None,
                        stderr: Some(format!("run exceeded timeout of {secs}s")),
                    });
                }
            }
        }
        _ => collect
            .await
            .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?,
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let command_output = CommandRunOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: (!stdout.is_empty()).then(|| stdout.to_string()),
        stderr: (!stderr.is_empty()).then(|| stderr.to_string()),
    };

    tracing::debug!(
        "{worker_id}: stdout for run {run_id}: {}",
        command_output.stdout.as_deref().unwrap_or_default()
    );
    if let Some(stderr) = &command_output.stderr {
        tracing::debug!("{worker_id}: stderr for run {run_id}: {stderr}");
    }

    Ok(command_output)
}

// HTTP runner: the response body is the typed `result` (with its Content-Type) on
// 2xx, or the typed `error` on non-2xx. A transport error (DNS/connect/timeout) is
// retryable; so are the conventionally-transient statuses (408/425/429/5xx).
async fn execute_http_request(
    worker_id: Uuid,
    run_id: Uuid,
    method: &str,
    url: &str,
    headers: &HashMap<String, String>,
    body: Option<&str>,
    timeout_sec: Option<u32>,
) -> Result<RunOutcome> {
    let http_method = reqwest::Method::from_bytes(method.to_uppercase().as_bytes())
        .map_err(|e| ArbiterError::ExecutionError(format!("invalid HTTP method '{method}': {e}")))?;

    let mut req = reqwest::Client::new().request(http_method, url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    if let Some(body) = body {
        req = req.body(body.to_string());
    }
    if let Some(secs) = timeout_sec {
        req = req.timeout(std::time::Duration::from_secs(secs as u64));
    }

    let resp = match req.send().await {
        Ok(resp) => resp,
        Err(e) => {
            // transport-level failure: connection/timeout/DNS are transient -> retryable.
            return Ok(RunOutcome {
                status: Some(ResultStatus::Retryable),
                exit_code: Some(-1),
                error: Some(format!("request error: {e}")),
                error_media_type: Some("text/plain".to_string()),
                ..Default::default()
            });
        }
    };

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp.text().await.unwrap_or_default();
    tracing::debug!("{worker_id}: http run {run_id} -> {status}");

    if status.is_success() {
        Ok(RunOutcome {
            status: Some(ResultStatus::Success),
            exit_code: Some(status.as_u16() as i32),
            result: (!body.is_empty()).then_some(body),
            result_media_type: content_type,
            ..Default::default()
        })
    } else {
        let retryable = matches!(status.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504);
        Ok(RunOutcome {
            status: Some(if retryable {
                ResultStatus::Retryable
            } else {
                ResultStatus::Failed
            }),
            exit_code: Some(status.as_u16() as i32),
            error: (!body.is_empty()).then_some(body),
            error_media_type: content_type,
            ..Default::default()
        })
    }
}
