//! End-to-end runner tests on the embedded SQLite backend: create a job, materialize
//! a due run, run one worker tick (claim + build snapshot + spawn execution), and
//! assert the run actually reached a terminal state with the expected output.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration as StdDuration;

use arbiter_core::{
    JobRun, JobRunState, MisfirePolicy, RunnerConfig, Store, WorkerConfig,
};
use arbiter_store_sqlite::SqliteStore;
use arbiter_worker::worker_tick;
use chrono::{Duration, Utc};
use uuid::Uuid;

async fn fresh_store() -> Arc<dyn Store + Send + Sync> {
    let path = std::env::temp_dir().join(format!("arbiter_flow_{}.db", Uuid::new_v4().simple()));
    let store = SqliteStore::connect(path.to_str().expect("utf-8 temp path"))
        .await
        .expect("SqliteStore::connect");
    Arc::new(store)
}

fn worker_cfg() -> WorkerConfig {
    WorkerConfig {
        worker_id: Uuid::new_v4(),
        display_name: "test".to_string(),
        capacity: 4,
        hostname: "test".to_string(),
        tick_interval_ms: 50,
        heartbeat_interval_ms: 1_000,
        dead_after_secs: 30,
        restart_count: 0,
        version: "test".to_string(),
        run_retention_secs: 0,
        prune_interval_secs: 3_600,
    }
}

/// Poll the job's most recent run until it reaches a terminal state.
async fn await_terminal(store: &Arc<dyn Store + Send + Sync>, job_id: Uuid) -> JobRun {
    for _ in 0..200 {
        let runs = store
            .list_recent_runs(None, None, None, Some(job_id), None)
            .await
            .expect("list_recent_runs");
        if let Some(run) = runs.first() {
            if matches!(
                run.state,
                JobRunState::Succeeded | JobRunState::Failed | JobRunState::Cancelled
            ) {
                return run.clone();
            }
        }
        tokio::time::sleep(StdDuration::from_millis(25)).await;
    }
    panic!("run never reached a terminal state");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_runner_full_flow() {
    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(
            "echo-job",
            None,
            RunnerConfig::Shell {
                command: "echo hello-from-shell".to_string(),
                working_dir: None,
            },
            1,
            MisfirePolicy::RunImmediately,
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?}",
        run.state
    );
    assert!(
        run.output.unwrap_or_default().contains("hello-from-shell"),
        "shell output should be captured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_runner_full_flow() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/ping"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("pong"))
        .mount(&server)
        .await;

    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(
            "http-job",
            None,
            RunnerConfig::Http {
                method: "GET".to_string(),
                url: format!("{}/ping", server.uri()),
                headers: None,
                body: None,
                timeout_sec: Some(5),
            },
            1,
            MisfirePolicy::RunImmediately,
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?}",
        run.state
    );
    assert_eq!(run.output.as_deref(), Some("pong"));
}
