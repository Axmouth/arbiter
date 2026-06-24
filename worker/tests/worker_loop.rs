//! Deterministic tests for the event-driven worker claim loop. Time is virtual: the
//! loop's `Clock` and the mock store's due-ness both read tokio's paused clock, so
//! advancing `tokio::time` moves "now" in lockstep with the loop's sleeps.
//!
//! The mock's `claim_job_runs` returns empty (it only records *when* a due run would have
//! been claimed), so the loop never executes anything - this isolates wake/claim timing.
//! Every other `Store` method is `unimplemented!()` and never reached on this path.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use tokio::sync::Notify;
use tokio::time::Instant;
use uuid::Uuid;

use arbiter_core::{
    ApiStore, Clock, ConfigStore, DbEngine, JobRun, JobRunState, JobSpec, JobStore, MisfirePolicy,
    Result, RetryConfig, RunOutcome, RunStore, RunnerConfig, RuntimeDefaults, RuntimeSettings,
    SecretMeta, SecretStore, Setting, SettingsStore, SharedDbConfig, Store, StoredKekShare,
    StoredKekVersion, StoredNodeKey, StoredSecret, Tenant, TenantStore, User, UserRole,
    WorkerConfig, WorkerRecord, WorkerStore,
};
use arbiter_worker::run_worker_loop;

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

/// Minimal store for the claim loop. `due` is the next claimable time; once virtual time
/// reaches it, the next claim "consumes" it and records `claimed_at` (the clock reading at
/// that moment). `claim_job_runs` returns empty so nothing executes.
struct MockStore {
    clock: Arc<VirtualClock>,
    due: Mutex<Option<DateTime<Utc>>>,
    claimed_at: Mutex<Option<DateTime<Utc>>>,
    runs_notify: Notify,
}

impl MockStore {
    fn new(clock: Arc<VirtualClock>, due: Option<DateTime<Utc>>) -> Self {
        Self {
            clock,
            due: Mutex::new(due),
            claimed_at: Mutex::new(None),
            runs_notify: Notify::new(),
        }
    }
    fn set_due(&self, at: Option<DateTime<Utc>>) {
        *self.due.lock().expect("lock") = at;
    }
    fn claimed_at(&self) -> Option<DateTime<Utc>> {
        *self.claimed_at.lock().expect("lock")
    }
}

impl Store for MockStore {}

#[async_trait]
impl RunStore for MockStore {
    async fn claim_job_runs(&self, _worker_id: Uuid, _limit: u32) -> Result<Vec<JobRun>> {
        let now = self.clock.now();
        let due = *self.due.lock().expect("lock");
        if let Some(d) = due {
            if now >= d {
                *self.due.lock().expect("lock") = None;
                *self.claimed_at.lock().expect("lock") = Some(now);
            }
        }
        Ok(vec![]) // never execute; this test only checks claim timing
    }
    async fn next_claimable_at(&self) -> Result<Option<DateTime<Utc>>> {
        Ok(*self.due.lock().expect("lock"))
    }
    async fn await_runs_change(&self) {
        self.runs_notify.notified().await;
    }
    async fn finalize_run(&self, _: Uuid, _: JobRunState, _: RunOutcome) -> Result<()> {
        unimplemented!()
    }
    async fn reschedule_for_retry(
        &self,
        _: Uuid,
        _: u32,
        _: DateTime<Utc>,
        _: RunOutcome,
    ) -> Result<()> {
        unimplemented!()
    }
    async fn prune_runs(&self, _older_than: DateTime<Utc>) -> Result<u64> {
        unimplemented!()
    }
}

#[async_trait]
impl WorkerStore for MockStore {
    async fn heartbeat(&self, _worker: &WorkerRecord) -> Result<()> {
        Ok(())
    }
    async fn reclaim_dead_workers_jobs(&self, _dead_after_secs: u32) -> Result<u64> {
        Ok(0)
    }
    async fn am_i_leader(&self) -> Result<bool> {
        Ok(false)
    }
    async fn lookup_by_id(&self, _: Uuid) -> Result<Option<(String, u32)>> {
        unimplemented!()
    }
    async fn incr_restart_count(&self, _: Uuid, _: &str) -> Result<u32> {
        unimplemented!()
    }
    async fn insert_worker(&self, _: Uuid, _: &str, _: &str, _: &str, _: u32) -> Result<()> {
        unimplemented!()
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

// The remaining Store supertraits are never exercised by the claim loop on this path.
#[async_trait]
impl JobStore for MockStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>> {
        unimplemented!()
    }
    async fn insert_job_run_if_missing(&self, _: Uuid, _: DateTime<Utc>) -> Result<bool> {
        unimplemented!()
    }
    async fn job_tenant(&self, _: Uuid) -> Result<Option<Uuid>> {
        unimplemented!()
    }
}

#[async_trait]
impl ApiStore for MockStore {
    async fn health_check(&self) -> Result<()> {
        unimplemented!()
    }
    async fn get_job(&self, _: Uuid, _: Option<Uuid>) -> Result<JobSpec> {
        unimplemented!()
    }
    #[allow(clippy::too_many_arguments)]
    async fn create_job(
        &self,
        _: Uuid,
        _: &str,
        _: Option<String>,
        _: RunnerConfig,
        _: u32,
        _: MisfirePolicy,
        _: RetryConfig,
    ) -> Result<JobSpec> {
        unimplemented!()
    }
    async fn list_jobs(&self, _: Option<Uuid>) -> Result<Vec<JobSpec>> {
        unimplemented!()
    }
    async fn list_recent_runs(
        &self,
        _: Option<u32>,
        _: Option<DateTime<Utc>>,
        _: Option<DateTime<Utc>>,
        _: Option<Uuid>,
        _: Option<Uuid>,
        _: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        unimplemented!()
    }
    async fn set_job_enabled(&self, _: Uuid, _: bool) -> Result<()> {
        unimplemented!()
    }
    async fn enable_job(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    async fn disable_job(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    #[allow(clippy::too_many_arguments)]
    async fn update_job(
        &self,
        _: Uuid,
        _: Option<String>,
        _: Option<Option<String>>,
        _: Option<RunnerConfig>,
        _: Option<u32>,
        _: Option<MisfirePolicy>,
        _: Option<RetryConfig>,
    ) -> Result<JobSpec> {
        unimplemented!()
    }
    async fn delete_job(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    async fn create_adhoc_run(&self, _: Uuid) -> Result<JobRun> {
        unimplemented!()
    }
    async fn cancel_run(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    async fn set_job_env(&self, _: Uuid, _: HashMap<String, String>) -> Result<()> {
        unimplemented!()
    }
    async fn get_job_env(&self, _: Uuid) -> Result<HashMap<String, String>> {
        unimplemented!()
    }
    async fn list_workers(&self) -> Result<Vec<WorkerRecord>> {
        unimplemented!()
    }
    async fn get_user_by_username(&self, _: &str) -> Result<User> {
        unimplemented!()
    }
    async fn get_user_by_id(&self, _: Uuid) -> Result<User> {
        unimplemented!()
    }
    async fn create_user(&self, _: &str, _: &str, _: UserRole, _: Option<Uuid>) -> Result<User> {
        unimplemented!()
    }
    async fn list_users(&self) -> Result<Vec<User>> {
        unimplemented!()
    }
    async fn delete_user(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    async fn update_password(&self, _: Uuid, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn update_user(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<UserRole>,
    ) -> Result<User> {
        unimplemented!()
    }
    async fn count_users(&self) -> Result<u32> {
        unimplemented!()
    }
}

#[async_trait]
impl SecretStore for MockStore {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_secret(
        &self,
        _: Uuid,
        _: &str,
        _: &[u8],
        _: &[u8],
        _: &str,
        _: &[u8],
        _: u32,
    ) -> Result<Uuid> {
        unimplemented!()
    }
    async fn get_secret_by_name(&self, _: Uuid, _: &str) -> Result<Option<StoredSecret>> {
        unimplemented!()
    }
    async fn get_secret(&self, _: Uuid, _: Option<Uuid>) -> Result<Option<StoredSecret>> {
        unimplemented!()
    }
    async fn list_secret_names(&self, _: Option<Uuid>) -> Result<Vec<SecretMeta>> {
        unimplemented!()
    }
    async fn delete_secret(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
    async fn insert_kek_version(&self, _: u32, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn list_kek_versions(&self) -> Result<Vec<StoredKekVersion>> {
        unimplemented!()
    }
    async fn set_kek_version_state(&self, _: u32, _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn rewrap_secret(&self, _: Uuid, _: &[u8], _: u32) -> Result<()> {
        unimplemented!()
    }
    async fn put_kek_share(&self, _: u32, _: Uuid, _: &[u8]) -> Result<()> {
        unimplemented!()
    }
    async fn get_kek_share(&self, _: u32, _: Uuid) -> Result<Option<StoredKekShare>> {
        unimplemented!()
    }
    async fn upsert_node_key(&self, _: Uuid, _: u32, _: &[u8], _: &str) -> Result<()> {
        unimplemented!()
    }
    async fn list_node_keys(&self) -> Result<Vec<StoredNodeKey>> {
        unimplemented!()
    }
}

#[async_trait]
impl TenantStore for MockStore {
    async fn create_tenant(&self, _: &str) -> Result<Tenant> {
        unimplemented!()
    }
    async fn get_tenant(&self, _: Uuid) -> Result<Option<Tenant>> {
        unimplemented!()
    }
    async fn list_tenants(&self) -> Result<Vec<Tenant>> {
        unimplemented!()
    }
}

#[async_trait]
impl ConfigStore for MockStore {
    #[allow(clippy::too_many_arguments)]
    async fn create_db_config(
        &self,
        _: Uuid,
        _: DbEngine,
        _: &str,
        _: &str,
        _: u16,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<SharedDbConfig> {
        unimplemented!()
    }
    async fn get_db_config(&self, _: Uuid, _: Option<Uuid>) -> Result<Option<SharedDbConfig>> {
        unimplemented!()
    }
    async fn list_db_configs(&self, _: Option<Uuid>) -> Result<Vec<SharedDbConfig>> {
        unimplemented!()
    }
    #[allow(clippy::too_many_arguments)]
    async fn update_db_config(
        &self,
        _: Uuid,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<u16>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
    ) -> Result<SharedDbConfig> {
        unimplemented!()
    }
    async fn delete_db_config(&self, _: Uuid) -> Result<()> {
        unimplemented!()
    }
}

fn defaults() -> RuntimeDefaults {
    RuntimeDefaults {
        misfire_catchup_secs: 0,
        run_retention_secs: 0, // skip the retention/leader path entirely
        prune_interval_secs: 3600,
        scheduler_backstop_secs: 180,
        worker_claim_backstop_secs: 300,
    }
}

fn worker_cfg() -> WorkerConfig {
    WorkerConfig {
        worker_id: Uuid::new_v4(),
        display_name: "test".into(),
        capacity: 4,
        hostname: "test".into(),
        tick_interval_ms: 200,
        heartbeat_interval_ms: 2_000,
        dead_after_secs: 30,
        restart_count: 0,
        version: "test".into(),
    }
}

async fn settle() {
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn spawn(store: Arc<MockStore>, clock: Arc<VirtualClock>) {
    let settings = RuntimeSettings::new(store.clone(), defaults());
    let dyn_clock: Arc<dyn Clock> = clock;
    tokio::spawn(async move {
        run_worker_loop(store, worker_cfg(), None, settings, dyn_clock).await;
    });
}

#[tokio::test(start_paused = true)]
async fn claims_at_the_due_time_not_before() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let clock = Arc::new(VirtualClock {
        base,
        start: Instant::now(),
    });
    let due = base + chrono::Duration::seconds(100);
    let store = Arc::new(MockStore::new(clock.clone(), Some(due)));
    spawn(store.clone(), clock);

    settle().await;
    assert!(
        store.claimed_at().is_none(),
        "must not claim before the run is due (it sleeps to the due time)"
    );

    tokio::time::advance(Duration::from_secs(100)).await;
    settle().await;

    let claimed = store.claimed_at().expect("claimed once due");
    assert!(
        (claimed - due).num_seconds().abs() <= 2,
        "claims right at the due time ({due} vs {claimed})"
    );
}

#[tokio::test(start_paused = true)]
async fn wakes_on_runs_notification() {
    let base = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let clock = Arc::new(VirtualClock {
        base,
        start: Instant::now(),
    });
    // Nothing due: the loop parks on the (5 min) backstop.
    let store = Arc::new(MockStore::new(clock.clone(), None));
    spawn(store.clone(), clock.clone());

    settle().await;
    assert!(store.claimed_at().is_none());

    // A run appears now and fires the notification - no time advanced.
    store.set_due(Some(clock.now()));
    store.runs_notify.notify_waiters();
    settle().await;

    assert!(
        store.claimed_at().is_some(),
        "a runs notification wakes the loop to claim without waiting out the backstop"
    );
}
