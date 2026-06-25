//! End-to-end runner tests on the embedded SQLite backend: create a job, materialize
//! a due run, run one worker tick (claim + build snapshot + spawn execution), and
//! assert the run actually reached a terminal state with the expected output.

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration as StdDuration;

use arbiter_core::{
    BackoffStrategy, DEFAULT_TENANT_ID, JobRun, JobRunState, MisfirePolicy, RetryConfig,
    RunnerConfig, SecretResolver, SecretStore, Store, WorkerConfig,
};
use arbiter_secrets::{NodeKeyring, SecretManager};
use arbiter_store_sqlite::SqliteStore;
use arbiter_worker::worker_tick;
use chrono::{Duration, Utc};
use std::collections::HashMap;
use uuid::Uuid;

async fn fresh_store() -> Arc<dyn Store + Send + Sync> {
    let path = std::env::temp_dir().join(format!("arbiter_flow_{}.db", Uuid::new_v4().simple()));
    let store = SqliteStore::connect(path.to_str().expect("utf-8 temp path"))
        .await
        .expect("SqliteStore::connect");
    Arc::new(store)
}

/// Set a single per-job env var through the Store API.
async fn set_job_env(store: &Arc<dyn Store + Send + Sync>, job_id: Uuid, key: &str, value: &str) {
    let mut env = HashMap::new();
    env.insert(key.to_string(), value.to_string());
    store.set_job_env(job_id, env).await.expect("set_job_env");
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
    }
}

/// Poll the job's most recent run until it reaches a terminal state.
async fn await_terminal(store: &Arc<dyn Store + Send + Sync>, job_id: Uuid) -> JobRun {
    for _ in 0..200 {
        let runs = store
            .list_recent_runs(None, None, None, Some(job_id), None, None)
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
        .create_job(DEFAULT_TENANT_ID, 
            "echo-job",
            None,
            RunnerConfig::Shell {
                command: "echo hello-from-shell".to_string(),
                working_dir: None,
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?}",
        run.state
    );
    assert!(
        run.stdout.unwrap_or_default().contains("hello-from-shell"),
        "shell output should be captured"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_runner_retries_on_tempfail() {
    // exit 75 (EX_TEMPFAIL) is retryable; with max_attempts=2 and zero backoff the
    // run is requeued once and then fails, ending at attempt 2.
    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "retry-job",
            None,
            RunnerConfig::Shell {
                command: "exit 75".to_string(),
                working_dir: None,
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig {
                max_attempts: 2,
                backoff_strategy: BackoffStrategy::Fixed,
                backoff_base_secs: 0,
                backoff_cap_secs: 0,
            },
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    // Tick repeatedly: claim -> retryable -> requeue (attempt 2) -> claim -> fail.
    let mut terminal = None;
    for _ in 0..40 {
        worker_tick(store.clone(), &cfg, &running, &None)
            .await
            .expect("worker_tick");
        tokio::time::sleep(StdDuration::from_millis(25)).await;
        let runs = store
            .list_recent_runs(None, None, None, Some(job.id), None, None)
            .await
            .expect("list_recent_runs");
        if let Some(r) = runs.first() {
            if matches!(r.state, JobRunState::Failed | JobRunState::Succeeded) {
                terminal = Some(r.clone());
                break;
            }
        }
    }

    let run = terminal.expect("run should reach a terminal state");
    assert!(
        matches!(run.state, JobRunState::Failed),
        "exhausted retries should fail, got {:?}",
        run.state
    );
    assert_eq!(run.attempt, 2, "should have run twice (attempt 2)");
}

/// Skip a test gracefully if an interpreter is not installed in the environment.
fn has_binary(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write a runner module into a fresh temp dir and return (dir, dir_as_str).
fn write_module(file_name: &str, contents: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("arbiter_mod_{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("create module dir");
    std::fs::write(dir.join(file_name), contents).expect("write module");
    dir
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_runner_full_flow() {
    if !has_binary("python3") {
        eprintln!("skipping python_runner_full_flow: python3 not found");
        return;
    }

    // A module exposing a task class whose run(ctx) returns a value; the runtime
    // marshals the return value into the structured result (no stdout scraping).
    let dir = write_module(
        "mytask.py",
        "class MyTask:\n    def run(self, ctx):\n        ctx.log.info('working')\n        return 'hello-from-python'\n",
    );

    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "py-job",
            None,
            RunnerConfig::Python {
                module: "mytask".to_string(),
                class_name: "MyTask".to_string(),
                timeout_sec: Some(30),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    // PYTHONPATH makes `from mytask import MyTask` resolve to our temp module.
    set_job_env(&store, job.id, "PYTHONPATH", dir.to_str().unwrap()).await;
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?} (err: {:?})",
        run.state,
        run.error
    );
    assert_eq!(run.result.as_deref(), Some("hello-from-python"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_runner_structured_output() {
    if !has_binary("python3") {
        eprintln!("skipping python_runner_structured_output: python3 not found");
        return;
    }

    // Returning a dict is marshaled into structured output (json), not stdout text.
    let dir = write_module(
        "mytask.py",
        "def run(ctx):\n    return {'rows': 42}\n",
    );

    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "py-struct",
            None,
            RunnerConfig::Python {
                module: "mytask".to_string(),
                class_name: "run".to_string(),
                timeout_sec: Some(30),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    set_job_env(&store, job.id, "PYTHONPATH", dir.to_str().unwrap()).await;
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(matches!(run.state, JobRunState::Succeeded));
    let out = run.result.unwrap_or_default();
    assert!(out.contains("rows") && out.contains("42"), "structured output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_runner_failure_is_structured() {
    if !has_binary("node") {
        eprintln!("skipping node_runner_failure_is_structured: node not found");
        return;
    }

    // A thrown error becomes a failed run with a structured error message.
    let dir = write_module(
        "mytask.js",
        "module.exports.run = function () { throw new Error('boom'); };\n",
    );

    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "node-fail",
            None,
            RunnerConfig::Node {
                module: "mytask".to_string(),
                function_name: "run".to_string(),
                timeout_sec: Some(30),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    set_job_env(&store, job.id, "NODE_PATH", dir.to_str().unwrap()).await;
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Failed),
        "expected Failed, got {:?}",
        run.state
    );
    assert!(
        run.error.unwrap_or_default().contains("boom"),
        "structured error should carry the message"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn node_runner_full_flow() {
    if !has_binary("node") {
        eprintln!("skipping node_runner_full_flow: node not found");
        return;
    }

    // A module exposing a run(ctx) that returns a value; the runtime marshals the
    // return value into the structured result (no stdout scraping).
    let dir = write_module(
        "mytask.js",
        "module.exports.run = function (ctx) { ctx.log.info('working'); return 'hello-from-node'; };\n",
    );

    let store = fresh_store().await;
    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");

    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "node-job",
            None,
            RunnerConfig::Node {
                module: "mytask".to_string(),
                function_name: "run".to_string(),
                timeout_sec: Some(30),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    // NODE_PATH makes `require('mytask')` resolve to our temp module.
    set_job_env(&store, job.id, "NODE_PATH", dir.to_str().unwrap()).await;
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?} (err: {:?})",
        run.state,
        run.error
    );
    assert_eq!(run.result.as_deref(), Some("hello-from-node"));
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
        .create_job(DEFAULT_TENANT_ID, 
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
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &None)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?}",
        run.state
    );
    assert_eq!(run.result.as_deref(), Some("pong"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn python_runner_resolves_secret_env() {
    if !has_binary("python3") {
        eprintln!("skipping python_runner_resolves_secret_env: python3 not found");
        return;
    }

    let dir = write_module(
        "mytask.py",
        "import os\n\ndef run(ctx):\n    return os.environ.get('MY_SECRET', '')\n",
    );

    let path = std::env::temp_dir().join(format!("arbiter_flow_{}.db", Uuid::new_v4().simple()));
    let sqlite = Arc::new(
        SqliteStore::connect(path.to_str().expect("utf-8"))
            .await
            .expect("connect"),
    );
    let store: Arc<dyn Store + Send + Sync> = sqlite.clone();
    let secret_store: Arc<dyn SecretStore + Send + Sync> = sqlite.clone();

    let mgr = SecretManager::load_or_bootstrap(
        secret_store,
        Uuid::new_v4(),
        std::sync::Arc::new(NodeKeyring::generate()),
    )
    .await
    .expect("secret manager");
    mgr.set_secret(DEFAULT_TENANT_ID, "apikey", b"s3cr3t")
        .await
        .expect("set_secret");
    let resolver: arbiter_worker::Secrets =
        Some(Arc::new(mgr) as Arc<dyn SecretResolver + Send + Sync>);

    let cfg = worker_cfg();
    store
        .insert_worker(cfg.worker_id, "test", "test", "test", 0)
        .await
        .expect("insert_worker");
    let job = store
        .create_job(DEFAULT_TENANT_ID, 
            "py-secret",
            None,
            RunnerConfig::Python {
                module: "mytask".to_string(),
                class_name: "run".to_string(),
                timeout_sec: Some(30),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");

    let mut env = HashMap::new();
    env.insert("PYTHONPATH".to_string(), dir.to_str().unwrap().to_string());
    env.insert("MY_SECRET".to_string(), "secret:apikey".to_string());
    store.set_job_env(job.id, env).await.expect("set_job_env");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(5))
        .await
        .expect("materialize run");

    let running = Arc::new(AtomicU32::new(0));
    worker_tick(store.clone(), &cfg, &running, &resolver)
        .await
        .expect("worker_tick");

    let run = await_terminal(&store, job.id).await;
    assert!(
        matches!(run.state, JobRunState::Succeeded),
        "expected Succeeded, got {:?} ({:?})",
        run.state,
        run.error
    );
    // The `secret:apikey` env reference was resolved to the real value at run time.
    assert_eq!(run.result.as_deref(), Some("s3cr3t"));
}
