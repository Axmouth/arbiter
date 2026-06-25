use chrono::{DateTime, Duration, Utc};
use arbiter_core::{
    ArbiterError, Clock, ExecutableConfigSnapshotMeta, JobRun, JobRunState, ResultStatus, Result,
    RunOutcome, RuntimeSettings, SecretResolver, Store, WorkerConfig, WorkerRecord,
    jittered_backstop_secs, next_retry_delay, snooze,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::process::Command;
use uuid::Uuid;

/// Optional secret resolver shared with run execution. `None` disables secret refs.
pub type Secrets = Option<Arc<dyn SecretResolver + Send + Sync>>;

/// Prefix marking an env value or DB password as a secret reference (`secret:<name>`).
const SECRET_PREFIX: &str = "secret:";

/// Decrements the in-flight task counter when a spawned run finishes (any path).
struct RunGuard(Arc<AtomicU32>);
impl Drop for RunGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

// TODO: algo to determine job's "work units" over time? And worker capacity?
pub async fn run_worker_loop(
    store: Arc<dyn Store + Send + Sync>,
    cfg: WorkerConfig,
    secrets: Secrets,
    settings: Arc<RuntimeSettings>,
    clock: Arc<dyn Clock>,
) -> ! {
    // In-flight run tasks, so the worker honors its capacity instead of over-spawning.
    let running = Arc::new(AtomicU32::new(0));

    // Heartbeat + dead-worker reclaim run on their own cadence so the claim loop can
    // sleep for minutes when idle without ever looking dead.
    spawn_heartbeat(store.clone(), cfg.clone());

    let mut last_prune: Option<DateTime<Utc>> = None;
    loop {
        let now = clock.now();

        // Retention: the leader prunes old terminal runs on its own interval. Runtime
        // settings override the static config defaults (near-live via the cache).
        let retention_secs = settings.run_retention_secs();
        if retention_secs > 0 {
            let prune_interval = settings.prune_interval_secs();
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

        // Claim + spawn due runs up to capacity.
        if let Err(e) = worker_tick(store.clone(), &cfg, &running, &secrets).await {
            tracing::error!("{}: worker_tick error: {e:?}", cfg.worker_id);
        }

        // Sleep until the next run is due (so cron stays on time without polling), capped
        // by the idle backstop, and wake immediately on a run notification. At capacity we
        // recheck on a short floor since a finishing task frees a slot with no signal.
        let available = cfg.capacity.saturating_sub(running.load(Ordering::Relaxed));
        let floor = std::time::Duration::from_millis(cfg.tick_interval_ms.max(1));
        let wake = if available == 0 {
            now + Duration::milliseconds(cfg.tick_interval_ms as i64)
        } else {
            let next_due = store.next_claimable_at().await.unwrap_or(None);
            let backstop = jittered_backstop_secs(settings.worker_claim_backstop_secs(), 15);
            worker_next_wake(now, next_due, backstop)
        };
        // Floor the sleep so an overdue-but-unclaimable run (e.g. a job at max
        // concurrency) cannot spin the loop.
        let sleep_for = (wake - now).to_std().unwrap_or(std::time::Duration::ZERO).max(floor);
        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = store.await_runs_change() => {}
        }
    }
}

/// Heartbeat + dead-worker reclaim on a fixed cadence, independent of the claim loop's
/// (possibly long) idle sleeps, so liveness detection is never starved.
fn spawn_heartbeat(store: Arc<dyn Store + Send + Sync>, cfg: WorkerConfig) {
    tokio::spawn(async move {
        loop {
            let rec = WorkerRecord {
                id: cfg.worker_id,
                display_name: cfg.display_name.clone(),
                hostname: cfg.hostname.clone(),
                last_seen: Utc::now(),
                capacity: cfg.capacity,
                restart_count: cfg.restart_count,
                version: env!("CARGO_PKG_VERSION").to_string(),
            };
            if let Err(e) = store.heartbeat(&rec).await {
                tracing::error!("{}: heartbeat failed: {e:?}", cfg.worker_id);
            }
            // TODO: later, only do this on the leader/reaper node.
            if let Err(e) = store.reclaim_dead_workers_jobs(cfg.dead_after_secs).await {
                tracing::error!("{}: reclaim_dead_workers_jobs failed: {e:?}", cfg.worker_id);
            }
            snooze(std::time::Duration::from_millis(cfg.heartbeat_interval_ms), 30).await;
        }
    });
}

/// When the worker should next wake to claim: at the next run's due time, but never later
/// than the idle backstop (`0` = unbounded, rely on the notification). With nothing due
/// and no backstop it parks far out, waiting to be notified. Never returns the past.
fn worker_next_wake(
    now: DateTime<Utc>,
    next_due: Option<DateTime<Utc>>,
    backstop_secs: u64,
) -> DateTime<Utc> {
    let by_backstop =
        (backstop_secs > 0).then(|| now + Duration::seconds(backstop_secs as i64));
    let wake = match (next_due, by_backstop) {
        (Some(d), Some(b)) => d.min(b),
        (Some(d), None) => d,
        (None, Some(b)) => b,
        (None, None) => now + Duration::days(1),
    };
    wake.max(now)
}

pub async fn worker_tick(
    store: Arc<dyn Store + Sync + Send>,
    cfg: &WorkerConfig,
    running: &Arc<AtomicU32>,
    secrets: &Secrets,
) -> Result<()> {
    let available = cfg.capacity.saturating_sub(running.load(Ordering::Relaxed));
    if available == 0 {
        // At capacity; the caller decides when to recheck.
        return Ok(());
    }

    let runs = store.claim_job_runs(cfg.worker_id, available).await?;
    let runs_num = runs.len();

    let wid = cfg.worker_id;
    for run in runs {
        running.fetch_add(1, Ordering::Relaxed);
        spawn_run_task(store.clone(), wid, run, running.clone(), secrets.clone());
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
    secrets: Secrets,
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

        // The run's tenant scopes any secret references it resolves (I7).
        let tenant = store.job_tenant(run.job_id).await.unwrap_or(None);

        let run_output: Result<RunOutcome> = match snapshot.meta {
            ExecutableConfigSnapshotMeta::Shell {
                command,
                working_dir,
                env,
            } => match resolve_env(&env, &secrets, tenant).await {
                Ok(env) => {
                    let mut cmd = build_shell_command(&command);
                    if let Some(dir) = &working_dir {
                        cmd.current_dir(dir);
                    }
                    run_subprocess(&store, worker_id, run.id, cmd, &env, None)
                        .await
                        .map(process_outcome)
                }
                Err(e) => Err(e),
            },
            ExecutableConfigSnapshotMeta::Python {
                module,
                class_name,
                timeout_sec,
                env,
            } => match resolve_env(&env, &secrets, tenant).await {
                Ok(env) => {
                    execute_runtime(
                        &store,
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
                Err(e) => Err(e),
            },
            ExecutableConfigSnapshotMeta::Node {
                module,
                function_name,
                timeout_sec,
                env,
            } => match resolve_env(&env, &secrets, tenant).await {
                Ok(env) => {
                    execute_runtime(
                        &store,
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
                Err(e) => Err(e),
            },
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
            ExecutableConfigSnapshotMeta::PgSql {
                host,
                port,
                username,
                password_secret,
                database,
                query,
                timeout_sec,
            } => match resolve_ref(&secrets, tenant, &password_secret).await {
                Ok(password) => {
                    execute_pgsql_query(
                        worker_id, run.id, &host, port, &username, &password, &database, &query,
                        timeout_sec,
                    )
                    .await
                }
                Err(e) => Err(e),
            },
            ExecutableConfigSnapshotMeta::MySql {
                host,
                port,
                username,
                password_secret,
                database,
                query,
                timeout_sec,
            } => match resolve_ref(&secrets, tenant, &password_secret).await {
                Ok(password) => {
                    execute_mysql_query(
                        worker_id, run.id, &host, port, &username, &password, &database, &query,
                        timeout_sec,
                    )
                    .await
                }
                Err(e) => Err(e),
            },
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
                match store.get_job(run.job_id, None).await {
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
#[allow(clippy::too_many_arguments)]
async fn execute_runtime(
    store: &Arc<dyn Store + Send + Sync>,
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
    let raw = run_subprocess(store, worker_id, run_id, cmd, env, timeout_sec).await?;

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
    store: &Arc<dyn Store + Send + Sync>,
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
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;

    // Drain both pipes concurrently into shared buffers, so a child that fills one pipe while
    // we wait on the other (or on exit) cannot deadlock, and so we can flush partial output
    // for live viewing as it arrives.
    let out_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let err_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let out_reader = spawn_pipe_reader(child.stdout.take(), out_buf.clone());
    let err_reader = spawn_pipe_reader(child.stderr.take(), err_buf.clone());

    // Periodically push captured-so-far output to the store so a live run view updates as the
    // process runs (the final values are written by finalize_run). Skips a flush when nothing
    // new has accumulated, to avoid redundant writes and notifications.
    let flusher = {
        let store = store.clone();
        let out_buf = out_buf.clone();
        let err_buf = err_buf.clone();
        tokio::spawn(async move {
            let mut last = (0usize, 0usize);
            loop {
                tokio::time::sleep(OUTPUT_FLUSH_INTERVAL).await;
                let (o, e) = snapshot_buffers(&out_buf, &err_buf);
                if (o.len(), e.len()) == last {
                    continue;
                }
                last = (o.len(), e.len());
                let _ = store.update_run_output(run_id, opt(&o).as_deref(), opt(&e).as_deref()).await;
            }
        })
    };

    let mut timed_out = false;
    let exit_code = match timeout_sec {
        Some(secs) if secs > 0 => {
            match tokio::time::timeout(
                std::time::Duration::from_secs(secs as u64),
                child.wait(),
            )
            .await
            {
                Ok(res) => res
                    .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?
                    .code()
                    .unwrap_or(-1),
                Err(_) => {
                    let _ = child.kill().await;
                    timed_out = true;
                    -1
                }
            }
        }
        _ => child
            .wait()
            .await
            .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?
            .code()
            .unwrap_or(-1),
    };

    // The readers finish at EOF once the child's pipes close; then stop the flusher.
    let _ = out_reader.await;
    let _ = err_reader.await;
    flusher.abort();

    let (stdout, mut stderr) = snapshot_buffers(&out_buf, &err_buf);
    if timed_out {
        let msg = format!("run exceeded timeout of {}s", timeout_sec.unwrap_or(0));
        stderr = if stderr.is_empty() {
            msg
        } else {
            format!("{stderr}\n{msg}")
        };
    }

    tracing::debug!("{worker_id}: stdout for run {run_id}: {stdout}");
    if !stderr.is_empty() {
        tracing::debug!("{worker_id}: stderr for run {run_id}: {stderr}");
    }

    Ok(CommandRunOutput {
        exit_code,
        stdout: opt(&stdout),
        stderr: opt(&stderr),
    })
}

/// How often a running process's captured output is flushed to the store for live viewing.
const OUTPUT_FLUSH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// Drain a child pipe into a shared byte buffer until EOF. Returns immediately if the pipe is
/// absent. Raw bytes are kept (decoded lossily on read) so multibyte characters split across
/// reads are not mangled.
fn spawn_pipe_reader<R>(reader: Option<R>, buf: Arc<Mutex<Vec<u8>>>) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt as _;
        let Some(mut reader) = reader else { return };
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    })
}

/// Decode the current stdout/stderr buffers to lossy UTF-8 strings.
fn snapshot_buffers(out: &Mutex<Vec<u8>>, err: &Mutex<Vec<u8>>) -> (String, String) {
    let decode = |m: &Mutex<Vec<u8>>| {
        m.lock()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    (decode(out), decode(err))
}

/// `None` for empty output, else the owned string (so empty streams stay null in storage).
fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
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

/// Resolve a value that may be a `secret:<name>` reference, within the run's tenant.
/// Plain values pass through. A secret reference needs both a resolver and a tenant.
async fn resolve_ref(secrets: &Secrets, tenant: Option<Uuid>, value: &str) -> Result<String> {
    match value.strip_prefix(SECRET_PREFIX) {
        Some(name) => {
            let resolver = secrets.as_ref().ok_or_else(|| {
                ArbiterError::ExecutionError(
                    "secret reference present but no secret resolver is configured".to_string(),
                )
            })?;
            let tenant = tenant.ok_or_else(|| {
                ArbiterError::ExecutionError(
                    "secret reference present but the run has no tenant".to_string(),
                )
            })?;
            resolver.resolve_secret(tenant, name).await
        }
        None => Ok(value.to_string()),
    }
}

/// Resolve every env value (each may be a `secret:<name>` reference) before exec.
async fn resolve_env(
    env: &HashMap<String, String>,
    secrets: &Secrets,
    tenant: Option<Uuid>,
) -> Result<HashMap<String, String>> {
    let mut out = HashMap::with_capacity(env.len());
    for (k, v) in env {
        out.insert(k.clone(), resolve_ref(secrets, tenant, v).await?);
    }
    Ok(out)
}

/// A DB query execution failure: a timeout, or an sqlx error (kept for classification).
enum DbExecError {
    Timeout,
    Sql(sqlx::Error),
}

async fn run_with_timeout<F>(
    timeout_sec: Option<u32>,
    fut: F,
) -> std::result::Result<u64, DbExecError>
where
    F: Future<Output = std::result::Result<u64, sqlx::Error>>,
{
    match timeout_sec {
        Some(secs) if secs > 0 => {
            match tokio::time::timeout(std::time::Duration::from_secs(secs as u64), fut).await {
                Ok(res) => res.map_err(DbExecError::Sql),
                Err(_) => Err(DbExecError::Timeout),
            }
        }
        _ => fut.await.map_err(DbExecError::Sql),
    }
}

/// Map a DB execution result onto a `RunOutcome`. Success reports rows affected as
/// JSON. A query/constraint error fails; a connection/io/timeout error is retryable
/// (transient), so the retry policy can requeue it.
fn db_outcome(result: std::result::Result<u64, DbExecError>) -> RunOutcome {
    match result {
        Ok(rows_affected) => RunOutcome {
            status: Some(ResultStatus::Success),
            exit_code: Some(0),
            result: Some(format!("{{\"rows_affected\":{rows_affected}}}")),
            result_media_type: Some("application/json".to_string()),
            ..Default::default()
        },
        Err(DbExecError::Timeout) => RunOutcome {
            status: Some(ResultStatus::Retryable),
            exit_code: Some(-1),
            error: Some("query timed out".to_string()),
            error_media_type: Some("text/plain".to_string()),
            ..Default::default()
        },
        Err(DbExecError::Sql(e)) => {
            let message = e.to_string();
            let retryable = !matches!(e, sqlx::Error::Database(_));
            RunOutcome {
                status: Some(if retryable {
                    ResultStatus::Retryable
                } else {
                    ResultStatus::Failed
                }),
                exit_code: Some(-1),
                error: Some(message),
                error_media_type: Some("text/plain".to_string()),
                ..Default::default()
            }
        }
    }
}

// DB runners: connect to the target database with the resolved password and run the
// query. Success reports rows affected (result-set capture is a future enhancement).
#[allow(clippy::too_many_arguments)]
async fn execute_pgsql_query(
    worker_id: Uuid,
    run_id: Uuid,
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: &str,
    query: &str,
    timeout_sec: Option<u32>,
) -> Result<RunOutcome> {
    use sqlx::Connection as _;
    let opts = sqlx::postgres::PgConnectOptions::new()
        .host(host)
        .port(port)
        .username(username)
        .password(password)
        .database(database);
    let exec = async move {
        let mut conn = sqlx::postgres::PgConnection::connect_with(&opts).await?;
        let result = sqlx::query(query).execute(&mut conn).await;
        let _ = conn.close().await;
        result.map(|r| r.rows_affected())
    };
    tracing::debug!("{worker_id}: pgsql run {run_id} -> {host}:{port}/{database}");
    Ok(db_outcome(run_with_timeout(timeout_sec, exec).await))
}

#[allow(clippy::too_many_arguments)]
async fn execute_mysql_query(
    worker_id: Uuid,
    run_id: Uuid,
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    database: &str,
    query: &str,
    timeout_sec: Option<u32>,
) -> Result<RunOutcome> {
    use sqlx::Connection as _;
    let opts = sqlx::mysql::MySqlConnectOptions::new()
        .host(host)
        .port(port)
        .username(username)
        .password(password)
        .database(database);
    let exec = async move {
        let mut conn = sqlx::mysql::MySqlConnection::connect_with(&opts).await?;
        let result = sqlx::query(query).execute(&mut conn).await;
        let _ = conn.close().await;
        result.map(|r| r.rows_affected())
    };
    tracing::debug!("{worker_id}: mysql run {run_id} -> {host}:{port}/{database}");
    Ok(db_outcome(run_with_timeout(timeout_sec, exec).await))
}

#[cfg(test)]
mod db_tests {
    use super::*;

    fn parse_pg_url(url: &str) -> Option<(String, u16, String, String, String)> {
        let rest = url
            .strip_prefix("postgres://")
            .or_else(|| url.strip_prefix("postgresql://"))?;
        let (creds, hostpart) = rest.split_once('@')?;
        let (user, pass) = creds.split_once(':')?;
        let (hostport, db) = hostpart.split_once('/')?;
        let db = db.split('?').next().unwrap_or(db);
        let (host, port) = hostport.split_once(':')?;
        Some((
            host.to_string(),
            port.parse().ok()?,
            user.to_string(),
            pass.to_string(),
            db.to_string(),
        ))
    }

    #[tokio::test]
    async fn pgsql_runner_executes_and_classifies() {
        let url = match std::env::var("ARBITER_TEST_DATABASE_URL") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("skipping pgsql_runner: ARBITER_TEST_DATABASE_URL unset");
                return;
            }
        };
        let (host, port, user, pass, db) = parse_pg_url(&url).expect("parse pg url");
        let wid = Uuid::new_v4();

        let ok = execute_pgsql_query(wid, Uuid::new_v4(), &host, port, &user, &pass, &db, "SELECT 1", Some(10))
            .await
            .expect("exec");
        assert!(
            matches!(ok.status, Some(ResultStatus::Success)),
            "got {:?} ({:?})",
            ok.status,
            ok.error
        );

        let bad = execute_pgsql_query(wid, Uuid::new_v4(), &host, port, &user, &pass, &db, "NOT VALID SQL", Some(10))
            .await
            .expect("exec");
        assert!(
            matches!(bad.status, Some(ResultStatus::Failed)),
            "a query error should fail (not retry)"
        );

        let down = execute_pgsql_query(wid, Uuid::new_v4(), &host, 1, &user, &pass, &db, "SELECT 1", Some(5))
            .await
            .expect("exec");
        assert!(
            matches!(down.status, Some(ResultStatus::Retryable)),
            "an unreachable server should be retryable"
        );
    }
}
