use chrono::{DateTime, Duration, Utc};
use arbiter_core::{
    ArbiterError, ExecutableConfigSnapshotMeta, JobRun, JobRunState, Result, Store, WorkerConfig,
    WorkerRecord, snooze,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::io::{AsyncReadExt, BufReader};
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
        if cfg.run_retention_secs > 0 {
            let due = last_prune
                .map(|t| (now - t).num_seconds() as u64 >= cfg.prune_interval_secs)
                .unwrap_or(true);
            if due {
                match store.am_i_leader().await {
                    Ok(true) => {
                        let cutoff = now - Duration::seconds(cfg.run_retention_secs as i64);
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
                    .update_job_run_state(
                        run.id,
                        JobRunState::Failed,
                        None,
                        None,
                        Some("No config snapshot found, aborting run".to_string()),
                    )
                    .await;
            }
        };

        let run_output = match snapshot.meta {
            ExecutableConfigSnapshotMeta::Shell {
                command,
                working_dir,
                env,
            } => execute_shell_command(worker_id, run.id, &command).await,
            _ => {
                return Err(ArbiterError::ExecutionError(format!(
                    "Not Implemented {} yet",
                    snapshot.meta.type_of_str()
                )));
            }
        };

        match run_output {
            Ok(CommandRunOutput {
                exit_code: 0,
                error_output,
                output,
            }) => {
                store
                    .update_job_run_state(
                        run.id,
                        JobRunState::Succeeded,
                        Some(0),
                        output,
                        error_output,
                    )
                    .await
            }
            Ok(CommandRunOutput {
                exit_code,
                error_output,
                output,
            }) => {
                store
                    .update_job_run_state(
                        run.id,
                        JobRunState::Failed,
                        Some(exit_code),
                        output,
                        error_output,
                    )
                    .await
            }
            Err(_) => {
                store
                    .update_job_run_state(run.id, JobRunState::Failed, None, None, None)
                    .await
            }
        }
    });
}

pub struct CommandRunOutput {
    exit_code: i32,
    output: Option<String>,
    error_output: Option<String>,
}

// TODO: run configs and different runners
// TODO: such as ssh config, pg/mysql etc, http api calls, docker, kubernetes, etc.
// TODO: config setups are separate and can be shared between jobs, also tested on the UI side, as test connecting to db for a db job config, etc.
// TODO: This can conveniently cover real world use cases like running DB backups, calling webhooks, running remote jobs etc.
// TODO: Optionally "preload" runners/setup before running jobs a tiny bit before the actual run time, e.g. docker images, ssh connections, db connections, authentication steps, etc.
// TODO: Potentially start by grabbing jobs a few seconds earlier than scheduled time to allow preloading/setup time. And wait until scheduled time to actually run the job.
async fn execute_shell_command(
    worker_id: Uuid,
    run_id: Uuid,
    command: &str,
) -> Result<CommandRunOutput> {
    #[cfg(windows)]
    let mut cmd = Command::new("cmd");
    #[cfg(windows)]
    cmd.arg("/C").arg(command);

    #[cfg(not(windows))]
    let mut cmd = Command::new("sh");
    #[cfg(not(windows))]
    cmd.arg("-c").arg(command);

    // TODO: Timeout
    let mut cmd_run = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;

    let status = cmd_run
        .wait()
        .await
        .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;

    // TODO: handle breaking of tracing due to likely output characters in git bash?
    let mut output = None;
    if let Some(stdout) = &mut cmd_run.stdout {
        let mut out = Vec::new();
        BufReader::new(stdout)
            .read_to_end(&mut out)
            .await
            .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
        let out_str = String::from_utf8_lossy(&out);
        if !out_str.is_empty() {
            output = Some(out_str.to_string())
        }
    }

    let mut error_output = None;
    if let Some(stderr) = &mut cmd_run.stderr {
        let mut out = Vec::new();
        BufReader::new(stderr)
            .read_to_end(&mut out)
            .await
            .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
        let out_str = String::from_utf8_lossy(&out);
        if !out_str.is_empty() {
            error_output = Some(out_str.to_string())
        }
    }

    let command_output = CommandRunOutput {
        exit_code: status.code().unwrap_or(-1),
        output,
        error_output,
    };

    tracing::debug!(
        "{worker_id}: Output for run {run_id}: {}",
        command_output.output.as_deref().unwrap_or_default()
    );
    if let Some(error_output) = &command_output.error_output {
        tracing::debug!("{worker_id}: Error Output for run {run_id}: {error_output}");
    }

    Ok(command_output)
}
