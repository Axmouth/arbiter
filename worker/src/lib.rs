use chrono::{DateTime, Duration, Utc};
use dromio_core::{
    DromioError, JobRun, JobRunState, JobStore, Result, RunStore, SchedulerConfig, Store,
    WorkerConfig, WorkerRecord, WorkerStore,
};
use shell_words::split;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::sleep;
use uuid::Uuid;

pub async fn run_worker_loop(store: Arc<dyn Store + Send + Sync>, cfg: WorkerConfig) -> ! {
    let mut last_heartbeat: Option<DateTime<Utc>> = None;

    loop {
        let now = Utc::now();

        // Heartbeat periodically
        let needs_heartbeat = last_heartbeat
            .map(|t| (now - t).num_milliseconds() >= cfg.heartbeat_interval_ms as i64)
            .unwrap_or(true);

        if needs_heartbeat {
            let rec = WorkerRecord {
                id: cfg.worker_id,
                hostname: cfg.hostname.clone(),
                last_seen: now,
                capacity: cfg.capacity,
            };

            if let Err(e) = store.heartbeat(&rec).await {
                eprintln!("[worker] heartbeat failed: {e:?}");
            } else {
                last_heartbeat = Some(now);
            }

            // Reclaim dead workers’ jobs as part of “maintenance”
            // TODO: later, only do this on leader/reaper worker
            if let Err(e) = store.reclaim_dead_workers_jobs(cfg.dead_after_secs).await {
                eprintln!("[worker] reclaim_dead_workers_jobs failed: {e:?}");
            }

            // TODO: Prune older runs occasionally?
        }

        // Run a worker tick: claim + spawn jobs
        if let Err(e) = worker_tick(store.clone(), &cfg).await {
            eprintln!("[worker] worker_tick error: {e:?}");
        }

        sleep(std::time::Duration::from_millis(cfg.tick_interval_ms)).await;
    }
}

pub async fn worker_tick(store: Arc<dyn Store + Sync + Send>, cfg: &WorkerConfig) -> Result<()> {
    let available = cfg.capacity - count_local_running_tasks();
    if available == 0 {
        sleep(Duration::milliseconds(200).to_std().unwrap()).await;
        return Ok(());
    }

    let runs = store.claim_job_runs(cfg.worker_id, available).await?;
    let runs_num = runs.len();

    let wid = cfg.worker_id;
    for run in runs {
        let store = store.clone();
        spawn_run_task(store, wid, run);
    }

    if runs_num > 0 {
        println!(
            "[worker] tick at {},  claimed and spawned {} job runs",
            Utc::now(),
            runs_num
        );
    }

    Ok(())
}

fn spawn_run_task(store: Arc<dyn Store + Sync + Send>, worker_id: Uuid, run: JobRun) {
    tokio::spawn(async move {
        println!(
            "[worker {}: {}] starting job run {}, for Job {}, Scheduled for {}",
            worker_id,
            Utc::now(),
            run.id,
            run.job_id,
            run.scheduled_for
        );
        let status = execute_command(&run.command).await;

        match status {
            Ok(0) => {
                store
                    .update_job_run_state(run.id, JobRunState::Succeeded, Some(0))
                    .await
                    .ok();
            }
            Ok(exit) => {
                store
                    .update_job_run_state(run.id, JobRunState::Failed, Some(exit))
                    .await
                    .ok();
            }
            Err(_) => {
                store
                    .update_job_run_state(run.id, JobRunState::Failed, None)
                    .await
                    .ok();
            }
        }
    });
}

// TODO: run configs and different runners
// TODO: such as ssh config, pg/mysql etc, http api calls, docker, kubernetes, etc.
// TODO: config setups are separate and can be shared between jobs, also tested on the UI side, as test connecting to db for a db job config, etc.
// TODO: This can conveniently cover real world use cases like running DB backups, calling webhooks, running remote jobs etc.
// TODO: Optionally "preload" runners/setup before running jobs a tiny bit before the actual run time, e.g. docker images, ssh connections, db connections, authentication steps, etc.
// TODO: Potentially start by grabbing jobs a few seconds earlier than scheduled time to allow preloading/setup time. And wait until scheduled time to actually run the job.
async fn execute_command(command: &str) -> Result<i32> {
    let parts =
        split(command).map_err(|e| DromioError::ExecutionError(format!("parse error: {e}")))?;

    let (bin, args) = parts
        .split_first()
        .ok_or_else(|| DromioError::ExecutionError("empty command".into()))?;

    let mut cmd = Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| DromioError::ExecutionError(e.to_string()))?;

    if let Some(stdout) = &mut cmd.stdout {
        let mut reader = BufReader::new(stdout).lines();

        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| DromioError::ExecutionError(e.to_string()))?
        {
            println!("Output: {}", line);
        }
    }

    if let Some(stderr) = &mut cmd.stderr {
        let mut reader = BufReader::new(stderr).lines();

        while let Some(line) = reader
            .next_line()
            .await
            .map_err(|e| DromioError::ExecutionError(e.to_string()))?
        {
            eprintln!("Error Output: {}", line);
        }
    }

    let status = cmd
        .wait()
        .await
        .map_err(|e| DromioError::ExecutionError(e.to_string()))?;

    Ok(status.code().unwrap_or(-1))
}

fn count_local_running_tasks() -> u32 {
    // Placeholder implementation
    0
}
