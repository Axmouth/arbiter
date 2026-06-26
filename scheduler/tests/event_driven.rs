//! Deterministic tests for the event-driven scheduler loop. Time is virtual: the loop's
//! `Clock` is tied to tokio's paused clock, so advancing `tokio::time` advances "now" in
//! lockstep and the loop's sleeps fire without any real waiting.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;
use uuid::Uuid;

use arbiter_core::{
    BackoffStrategy, Clock, JobSpec, JobStore, MisfirePolicy, Result, RuntimeDefaults,
    RuntimeSettings, SchedulerConfig, Setting, SettingsStore, WorkerRecord, WorkerStore,
    RunnerConfig,
};
use arbiter_scheduler::run_scheduler_loop;

/// A clock pinned to tokio's (paused) virtual time: `now = base + virtual_elapsed`.
struct VirtualClock {
    base: DateTime<Utc>,
    start: Instant,
}

impl Clock for VirtualClock {
    fn now(&self) -> DateTime<Utc> {
        self.base + chrono::Duration::from_std(self.start.elapsed()).unwrap_or_default()
    }
}

/// In-memory store covering just what the scheduler loop touches, with controllable
/// leadership, a jobs-change signal, and recorded materializations.
struct MockStore {
    jobs: Vec<JobSpec>,
    inserted: Mutex<Vec<(Uuid, DateTime<Utc>)>>,
    ticks: AtomicU32,
    jobs_notify: Notify,
}

impl MockStore {
    fn new(jobs: Vec<JobSpec>) -> Self {
        Self {
            jobs,
            inserted: Mutex::new(Vec::new()),
            ticks: AtomicU32::new(0),
            jobs_notify: Notify::new(),
        }
    }
    fn insert_count(&self) -> usize {
        self.inserted.lock().expect("lock").len()
    }
    fn tick_count(&self) -> u32 {
        self.ticks.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl JobStore for MockStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>> {
        self.ticks.fetch_add(1, Ordering::SeqCst);
        Ok(self.jobs.clone())
    }
    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool> {
        let mut v = self.inserted.lock().expect("lock");
        if v.iter().any(|(j, t)| *j == job_id && *t == scheduled_for) {
            return Ok(false);
        }
        v.push((job_id, scheduled_for));
        Ok(true)
    }
    async fn job_tenant(&self, _job_id: Uuid) -> Result<Option<Uuid>> {
        Ok(None)
    }
    async fn await_jobs_change(&self) {
        self.jobs_notify.notified().await;
    }
}

#[async_trait]
impl WorkerStore for MockStore {
    async fn heartbeat(&self, _worker: &WorkerRecord) -> Result<()> {
        Ok(())
    }
    async fn lookup_by_id(&self, _id: Uuid) -> Result<Option<(String, u32)>> {
        Ok(None)
    }
    async fn incr_restart_count(&self, _id: Uuid, _version: &str) -> Result<u32> {
        Ok(0)
    }
    async fn insert_worker(
        &self,
        _id: Uuid,
        _display_name: &str,
        _hostname: &str,
        _version: &str,
        _restart_count: u32,
    ) -> Result<()> {
        Ok(())
    }
    async fn reclaim_dead_workers_jobs(&self, _dead_after_secs: u32) -> Result<u64> {
        Ok(0)
    }
    async fn am_i_leader(&self) -> Result<bool> {
        Ok(true)
    }
}

#[async_trait]
impl SettingsStore for MockStore {
    async fn get_setting(&self, _key: &str) -> Result<Option<String>> {
        Ok(None)
    }
    async fn set_setting(&self, _key: &str, _value: &str) -> Result<()> {
        Ok(())
    }
    async fn list_settings(&self) -> Result<Vec<Setting>> {
        Ok(vec![])
    }
}

fn every_minute_job() -> JobSpec {
    JobSpec {
        id: Uuid::new_v4(),
        name: "every-minute".into(),
        schedule_cron: Some("* * * * *".into()),
        enabled: true,
        runner_cfg: RunnerConfig::Shell {
            command: "echo hi".into(),
            working_dir: None,
        },
        max_concurrency: 1,
        misfire_policy: MisfirePolicy::RunImmediately,
        max_attempts: 1,
        backoff_strategy: BackoffStrategy::Exponential,
        backoff_base_secs: 30,
        backoff_cap_secs: 3600,
    }
}

fn defaults() -> RuntimeDefaults {
    RuntimeDefaults {
        misfire_catchup_secs: 0,
        run_retention_secs: 0,
        prune_interval_secs: 3600,
        scheduler_backstop_secs: 180,
        worker_claim_backstop_secs: 300,
        max_log_bytes: 0,
    }
}

fn scheduler_cfg() -> SchedulerConfig {
    SchedulerConfig {
        tick_interval_ms: 2_000,
        misfire_catchup_secs: 0,
    }
}

/// Let the spawned loop run its ready work without firing its (much longer) planned
/// sleep: a few 1ms virtual ticks only advance time by milliseconds.
async fn settle() {
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn spawn_loop(store: Arc<MockStore>) {
    let clock: Arc<dyn Clock> = Arc::new(VirtualClock {
        base: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        start: Instant::now(),
    });
    let settings = RuntimeSettings::new(store.clone(), defaults());
    let loop_store = store.clone();
    tokio::spawn(async move {
        run_scheduler_loop(loop_store, scheduler_cfg(), Uuid::new_v4(), settings, clock).await;
    });
}

#[tokio::test(start_paused = true)]
async fn replans_immediately_on_jobs_notification() {
    let store = Arc::new(MockStore::new(vec![every_minute_job()]));
    spawn_loop(store.clone());

    settle().await;
    let before = store.tick_count();
    assert!(before >= 1, "the loop runs an initial planning tick");

    // No time advanced; only the notification should rouse it to replan.
    store.jobs_notify.notify_waiters();
    settle().await;

    assert!(
        store.tick_count() > before,
        "a jobs-change notification triggers an immediate replan (no time passed)"
    );
}

#[tokio::test(start_paused = true)]
async fn materializes_further_fires_as_time_advances() {
    let store = Arc::new(MockStore::new(vec![every_minute_job()]));
    spawn_loop(store.clone());

    settle().await;
    let first = store.insert_count();
    assert!(first >= 1, "the initial tick materializes the imminent fire(s)");

    // Advance several minutes of virtual time; the loop should wake at each fire and
    // materialize more runs without any real waiting.
    tokio::time::advance(Duration::from_secs(180)).await;
    settle().await;

    assert!(
        store.insert_count() > first,
        "advancing time materializes further fires ({} -> {})",
        first,
        store.insert_count()
    );
}
