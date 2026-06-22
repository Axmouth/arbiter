//! Shared, backend-agnostic conformance suite for the arbiter `Store` trait.
//!
//! The suite is a registry of [`Case`]s (data); a runner crosses every backend
//! with every applicable case. The runner and the per-backend [`BackendFactory`]
//! implementations live in `tests/conformance.rs` (that is where, for example,
//! Postgres is set up). Nothing here knows about a concrete backend.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// `Store` brings its supertrait methods (ApiStore/JobStore/RunStore/WorkerStore)
// into scope for `dyn Store`, so only the trait and the data types are imported.
use arbiter_core::{
    DEFAULT_TENANT_ID, ExecutableConfigSnapshotMeta, JobRunState, MisfirePolicy, ResultStatus,
    RetryConfig, RunOutcome, RunnerConfig, Store, UserRole, WorkerRecord,
};
use chrono::{DateTime, Duration, Utc};
use futures::future::BoxFuture;
use uuid::Uuid;

/// An isolated store handle the suite exercises through the trait only.
pub type StoreRef = Arc<dyn Store + Send + Sync>;

/// One orthogonal property a backend may or may not provide. "Clustering" is two
/// axes on purpose: `MultiWorker` (concurrency correctness against one instance)
/// is independent of `MultiNode` (replication and failover across instances), so
/// single-node SQLite can run the bulk of the suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Durable,
    NativeUnique,
    MultiWorker,
    LeaderElection,
    MultiNode,
    Fencing,
    Retention,
}

/// What a backend promises to support. Drives which cases run vs are skipped.
#[derive(Debug, Clone, Copy)]
pub struct Capabilities {
    pub durable: bool,
    pub native_unique: bool,
    pub multi_worker: bool,
    pub leader_election: bool,
    pub multi_node: bool,
    pub fencing: bool,
    pub retention: bool,
}

impl Capabilities {
    pub fn has(&self, c: Capability) -> bool {
        match c {
            Capability::Durable => self.durable,
            Capability::NativeUnique => self.native_unique,
            Capability::MultiWorker => self.multi_worker,
            Capability::LeaderElection => self.leader_election,
            Capability::MultiNode => self.multi_node,
            Capability::Fencing => self.fencing,
            Capability::Retention => self.retention,
        }
    }

    pub fn satisfies(&self, needs: &[Capability]) -> bool {
        needs.iter().all(|c| self.has(*c))
    }
}

/// A handle to persistent storage that can be opened more than once, so durability
/// cases can write, drop the handle, reopen the same storage, and re-read.
#[async_trait::async_trait]
pub trait DurableHandle: Send + Sync {
    async fn open(&self) -> StoreRef;
}

/// A backend the suite can grade. Implemented once per store, in the test wiring,
/// where it also owns its resource lifecycle (container, temp dir, ...).
#[async_trait::async_trait]
pub trait BackendFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    /// A fresh, empty, isolated store.
    async fn fresh(&self) -> StoreRef;

    /// A reopenable handle to this backend's storage (durability group). Default:
    /// not durable, so durability cases are skipped.
    async fn durable_handle(&self) -> Option<Box<dyn DurableHandle>> {
        None
    }

    /// Two independent store handles onto the *same* backend instance (leadership
    /// group). Default: not available, so leadership cases are skipped.
    async fn paired(&self) -> Option<(StoreRef, StoreRef)> {
        None
    }
}

/// One behavioral case, expressed as data so the runner can cross it with backends.
pub struct Case {
    pub group: &'static str,
    pub name: &'static str,
    pub needs: &'static [Capability],
    pub run: fn(StoreRef) -> BoxFuture<'static, ()>,
}

/// The behavioral contract, as data. Add a case here and every capable backend
/// is graded against it automatically.
pub fn cases() -> Vec<Case> {
    vec![
        Case {
            group: "crud",
            name: "job_create_get",
            needs: &[],
            run: |s| Box::pin(crud_job_create_get(s)),
        },
        Case {
            group: "crud",
            name: "job_enable_disable_lists",
            needs: &[],
            run: |s| Box::pin(crud_job_enable_disable_lists(s)),
        },
        Case {
            group: "crud",
            name: "user_create_count",
            needs: &[],
            run: |s| Box::pin(crud_user_create_count(s)),
        },
        Case {
            group: "materialization",
            name: "insert_idempotent",
            needs: &[],
            run: |s| Box::pin(mat_insert_idempotent(s)),
        },
        Case {
            group: "materialization",
            name: "concurrent_insert_dedup",
            needs: &[Capability::MultiWorker],
            run: |s| Box::pin(mat_concurrent_dedup(s)),
        },
        Case {
            group: "claim",
            name: "only_due_runs_claimed",
            needs: &[],
            run: |s| Box::pin(claim_only_due(s)),
        },
        Case {
            group: "claim",
            name: "each_run_claimed_once",
            needs: &[Capability::MultiWorker],
            run: |s| Box::pin(claim_each_once(s)),
        },
        Case {
            group: "state",
            name: "transition_to_succeeded",
            needs: &[],
            run: |s| Box::pin(state_transition(s)),
        },
        Case {
            group: "listing",
            name: "recent_runs_desc_and_filter",
            needs: &[],
            run: |s| Box::pin(listing_recent(s)),
        },
        Case {
            group: "listing",
            name: "filter_by_worker",
            needs: &[],
            run: |s| Box::pin(listing_filter_by_worker(s)),
        },
        Case {
            group: "listing",
            name: "before_cursor_direction",
            needs: &[],
            run: |s| Box::pin(listing_before_cursor(s)),
        },
        Case {
            group: "claim",
            name: "respects_limit",
            needs: &[],
            run: |s| Box::pin(claim_respects_limit(s)),
        },
        Case {
            group: "claim",
            name: "sets_worker_and_running",
            needs: &[],
            run: |s| Box::pin(claim_sets_worker_and_running(s)),
        },
        Case {
            group: "claim",
            name: "skips_disabled_job_runs",
            needs: &[],
            run: |s| Box::pin(claim_skips_disabled(s)),
        },
        Case {
            group: "reaper",
            name: "requeues_dead_worker_runs",
            needs: &[],
            run: |s| Box::pin(reaper_requeues_dead(s)),
        },
        Case {
            group: "reaper",
            name: "spares_live_worker_runs",
            needs: &[],
            run: |s| Box::pin(reaper_spares_live(s)),
        },
        Case {
            group: "state",
            name: "cancel_prevents_claim",
            needs: &[],
            run: |s| Box::pin(state_cancel_prevents_claim(s)),
        },
        Case {
            group: "state",
            name: "adhoc_run_claimable",
            needs: &[],
            run: |s| Box::pin(state_adhoc_claimable(s)),
        },
        Case {
            group: "claim",
            name: "skips_deleted_job_runs",
            needs: &[],
            run: |s| Box::pin(claim_skips_deleted(s)),
        },
        Case {
            group: "claim",
            name: "orders_oldest_first",
            needs: &[],
            run: |s| Box::pin(claim_orders_oldest_first(s)),
        },
        Case {
            group: "reaper",
            name: "is_idempotent",
            needs: &[],
            run: |s| Box::pin(reaper_idempotent(s)),
        },
        Case {
            group: "listing",
            name: "after_cursor_direction",
            needs: &[],
            run: |s| Box::pin(listing_after_cursor(s)),
        },
        Case {
            group: "worker",
            name: "lookup_after_insert",
            needs: &[],
            run: |s| Box::pin(worker_lookup_after_insert(s)),
        },
        Case {
            group: "worker",
            name: "incr_restart_count",
            needs: &[],
            run: |s| Box::pin(worker_incr_restart_count(s)),
        },
        Case {
            group: "worker",
            name: "list_registered",
            needs: &[],
            run: |s| Box::pin(worker_list_registered(s)),
        },
        Case {
            group: "state",
            name: "failed_records_exit_and_error",
            needs: &[],
            run: |s| Box::pin(state_failed_records_exit_and_error(s)),
        },
        Case {
            group: "claim",
            name: "claimed_run_not_double_claimed",
            needs: &[],
            run: |s| Box::pin(claim_claimed_run_not_double_claimed(s)),
        },
        Case {
            group: "reaper",
            name: "ignores_queued_runs",
            needs: &[],
            run: |s| Box::pin(reaper_ignores_queued_runs(s)),
        },
        Case {
            group: "materialization",
            name: "distinct_jobs_independent",
            needs: &[],
            run: |s| Box::pin(mat_distinct_jobs_independent(s)),
        },
        Case {
            group: "crud",
            name: "update_job",
            needs: &[],
            run: |s| Box::pin(crud_update_job(s)),
        },
        Case {
            group: "crud",
            name: "update_user",
            needs: &[],
            run: |s| Box::pin(crud_update_user(s)),
        },
        Case {
            group: "retention",
            name: "prunes_old_terminal_runs",
            needs: &[Capability::Retention],
            run: |s| Box::pin(retention_prunes_old_terminal(s)),
        },
        Case {
            group: "retention",
            name: "spares_active_runs",
            needs: &[Capability::Retention],
            run: |s| Box::pin(retention_spares_active(s)),
        },
        Case {
            group: "settings",
            name: "set_get_roundtrip",
            needs: &[],
            run: |s| Box::pin(settings_set_get(s)),
        },
        Case {
            group: "settings",
            name: "set_overwrites",
            needs: &[],
            run: |s| Box::pin(settings_overwrite(s)),
        },
        Case {
            group: "settings",
            name: "list_returns_all",
            needs: &[],
            run: |s| Box::pin(settings_list(s)),
        },
        Case {
            group: "claim",
            name: "carries_shell_snapshot",
            needs: &[],
            run: |s| Box::pin(claim_carries_shell_snapshot(s)),
        },
        Case {
            group: "claim",
            name: "carries_http_snapshot",
            needs: &[],
            run: |s| Box::pin(claim_carries_http_snapshot(s)),
        },
        Case {
            group: "claim",
            name: "carries_python_snapshot",
            needs: &[],
            run: |s| Box::pin(claim_carries_python_snapshot(s)),
        },
        Case {
            group: "claim",
            name: "carries_node_snapshot",
            needs: &[],
            run: |s| Box::pin(claim_carries_node_snapshot(s)),
        },
        Case {
            group: "env",
            name: "set_get_roundtrip",
            needs: &[],
            run: |s| Box::pin(env_set_get_roundtrip(s)),
        },
        Case {
            group: "env",
            name: "set_replaces_all",
            needs: &[],
            run: |s| Box::pin(env_set_replaces_all(s)),
        },
        Case {
            group: "claim",
            name: "carries_env_snapshot",
            needs: &[],
            run: |s| Box::pin(claim_carries_env_snapshot(s)),
        },
        Case {
            group: "outcome",
            name: "records_result_and_status",
            needs: &[],
            run: |s| Box::pin(outcome_records_result(s)),
        },
        Case {
            group: "outcome",
            name: "records_structured_error",
            needs: &[],
            run: |s| Box::pin(outcome_records_error(s)),
        },
        Case {
            group: "retry",
            name: "reschedule_requeues_with_attempt",
            needs: &[],
            run: |s| Box::pin(retry_reschedule_requeues(s)),
        },
        Case {
            group: "secrets",
            name: "upsert_get_roundtrip",
            needs: &[],
            run: |s| Box::pin(secrets_roundtrip(s)),
        },
        Case {
            group: "secrets",
            name: "upsert_replaces_by_name",
            needs: &[],
            run: |s| Box::pin(secrets_replace(s)),
        },
        Case {
            group: "secrets",
            name: "delete_removes",
            needs: &[],
            run: |s| Box::pin(secrets_delete(s)),
        },
        Case {
            group: "secrets",
            name: "kek_version_and_share_roundtrip",
            needs: &[],
            run: |s| Box::pin(secrets_kek_roundtrip(s)),
        },
        Case {
            group: "secrets",
            name: "node_key_roundtrip",
            needs: &[],
            run: |s| Box::pin(secrets_node_key(s)),
        },
        Case {
            group: "secrets",
            name: "isolated_per_tenant",
            needs: &[],
            run: |s| Box::pin(secrets_tenant_isolation(s)),
        },
        Case {
            group: "tenant",
            name: "create_get_list",
            needs: &[],
            run: |s| Box::pin(tenant_create_get_list(s)),
        },
        Case {
            group: "tenant",
            name: "user_carries_tenant_scope",
            needs: &[],
            run: |s| Box::pin(tenant_user_scope(s)),
        },
    ]
}

/// A durability case: receives a reopenable handle instead of a single store.
pub struct DurableCase {
    pub group: &'static str,
    pub name: &'static str,
    pub run: fn(Box<dyn DurableHandle>) -> BoxFuture<'static, ()>,
}

/// Cases that require reopening storage. Run only for backends whose
/// `durable_handle()` is `Some`.
pub fn durable_cases() -> Vec<DurableCase> {
    vec![
        DurableCase {
            group: "durability",
            name: "definitions_survive_reopen",
            run: |h| Box::pin(durability_definitions_survive(h)),
        },
        DurableCase {
            group: "durability",
            name: "inflight_run_recoverable",
            run: |h| Box::pin(durability_inflight_run_recoverable(h)),
        },
    ]
}

/// A leadership case: receives two handles to the same backend instance.
pub struct LeadershipCase {
    pub group: &'static str,
    pub name: &'static str,
    pub run: fn((StoreRef, StoreRef)) -> BoxFuture<'static, ()>,
}

/// Cases that need two nodes against one backend. Run only when `paired()` is `Some`.
pub fn leadership_cases() -> Vec<LeadershipCase> {
    vec![
        LeadershipCase {
            group: "leadership",
            name: "single_leader_among_two",
            run: |p| Box::pin(leadership_single_leader(p)),
        },
        LeadershipCase {
            group: "leadership",
            name: "stable_across_calls",
            run: |p| Box::pin(leadership_stable_across_calls(p)),
        },
    ]
}

// --- helpers ---

fn shell() -> RunnerConfig {
    RunnerConfig::Shell {
        command: "echo hi".to_string(),
        working_dir: None,
    }
}

/// Create a job and (optionally) enable it. Materialization requires an enabled
/// job, so cases that insert runs seed with `enabled = true`.
async fn seed_job(store: &StoreRef, cron: Option<&str>, enabled: bool) -> Uuid {
    let job = store
        .create_job(
            "seed-job",
            cron.map(|c| c.to_string()),
            shell(),
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    if enabled {
        store.enable_job(job.id).await.expect("enable_job");
    }
    job.id
}

/// Register a worker so claims (which set `job_runs.worker_id`, an FK to `workers`)
/// have a valid owner.
async fn seed_worker(store: &StoreRef) -> Uuid {
    let id = Uuid::new_v4();
    store
        .insert_worker(id, "test-worker", "localhost", "test", 0)
        .await
        .expect("insert_worker");
    id
}

/// Drive a worker's `last_seen` directly (heartbeat writes the supplied value), so
/// liveness/reaper cases control time through data instead of sleeping.
async fn set_last_seen(store: &StoreRef, id: Uuid, last_seen: DateTime<Utc>) {
    let rec = WorkerRecord {
        id,
        display_name: "test-worker".to_string(),
        hostname: "localhost".to_string(),
        last_seen,
        capacity: 1,
        restart_count: 0,
        version: "test".to_string(),
    };
    store.heartbeat(&rec).await.expect("heartbeat");
}

// --- cases ---

async fn crud_job_create_get(store: StoreRef) {
    let job = store
        .create_job("alpha", None, shell(), 3, MisfirePolicy::RunImmediately, RetryConfig::default())
        .await
        .expect("create_job");
    let got = store.get_job(job.id).await.expect("get_job");
    assert_eq!(got.id, job.id);
    assert_eq!(got.name, "alpha");
    assert_eq!(got.max_concurrency, 3);
}

async fn crud_job_enable_disable_lists(store: StoreRef) {
    let job = store
        .create_job(
            "beta",
            Some("* * * * *".to_string()),
            shell(),
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");

    store.enable_job(job.id).await.expect("enable_job");
    let enabled = store
        .list_enabled_cron_jobs()
        .await
        .expect("list_enabled_cron_jobs");
    assert!(
        enabled.iter().any(|j| j.id == job.id),
        "enabled cron job should be listed"
    );

    store.disable_job(job.id).await.expect("disable_job");
    let enabled = store
        .list_enabled_cron_jobs()
        .await
        .expect("list_enabled_cron_jobs");
    assert!(
        !enabled.iter().any(|j| j.id == job.id),
        "disabled job should not be listed as enabled"
    );

    let all = store.list_jobs().await.expect("list_jobs");
    assert!(
        all.iter().any(|j| j.id == job.id),
        "job should still exist in the full listing"
    );
}

async fn crud_user_create_count(store: StoreRef) {
    assert_eq!(
        store.count_users().await.expect("count_users"),
        0,
        "a fresh store has no users"
    );
    store
        .create_user("admin", "hash", UserRole::Admin, None)
        .await
        .expect("create_user");
    assert_eq!(store.count_users().await.expect("count_users"), 1);

    let user = store
        .get_user_by_username("admin")
        .await
        .expect("get_user_by_username");
    assert_eq!(user.username, "admin");
    assert!(matches!(user.role, UserRole::Admin));
}

async fn mat_insert_idempotent(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let ts = Utc::now() - Duration::seconds(10);

    assert!(
        store
            .insert_job_run_if_missing(job, ts)
            .await
            .expect("insert_job_run_if_missing"),
        "first insert should create the run"
    );
    assert!(
        !store
            .insert_job_run_if_missing(job, ts)
            .await
            .expect("insert_job_run_if_missing"),
        "duplicate (job, scheduled_for) should be a no-op"
    );

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(runs.len(), 1);
}

async fn mat_concurrent_dedup(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let ts = Utc::now() - Duration::seconds(10);

    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        handles.push(tokio::spawn(async move {
            s.insert_job_run_if_missing(job, ts)
                .await
                .expect("insert_job_run_if_missing")
        }));
    }

    let mut inserted = 0;
    for h in handles {
        if h.await.expect("join") {
            inserted += 1;
        }
    }
    assert_eq!(inserted, 1, "exactly one concurrent insert should win");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(runs.len(), 1, "only one run row should exist");
}

async fn claim_only_due(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let due = Utc::now() - Duration::seconds(60);
    let not_yet = Utc::now() + Duration::seconds(3600);
    store
        .insert_job_run_if_missing(job, due)
        .await
        .expect("insert due run");
    store
        .insert_job_run_if_missing(job, not_yet)
        .await
        .expect("insert future run");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 10)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1, "only the due run should be claimable");
    assert!(
        claimed[0].scheduled_for <= Utc::now(),
        "claimed run must be due"
    );
}

async fn claim_each_once(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let n = 50usize;
    for i in 0..n {
        let ts = Utc::now() - Duration::seconds((i + 1) as i64);
        store
            .insert_job_run_if_missing(job, ts)
            .await
            .expect("insert run");
    }

    // Many workers race to claim the same pool of due runs.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = store.clone();
        let wid = seed_worker(&store).await;
        handles.push(tokio::spawn(async move {
            s.claim_job_runs(wid, n as u32)
                .await
                .expect("claim_job_runs")
        }));
    }

    let mut claimed = Vec::new();
    for h in handles {
        claimed.extend(h.await.expect("join"));
    }

    let ids: HashSet<Uuid> = claimed.iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), claimed.len(), "a run was claimed by two workers");
    assert_eq!(
        claimed.len(),
        n,
        "every due run should be claimed exactly once"
    );
}

async fn state_transition(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let ts = Utc::now() - Duration::seconds(30);
    store
        .insert_job_run_if_missing(job, ts)
        .await
        .expect("insert run");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let run = claimed[0].clone();
    assert!(
        matches!(run.state, JobRunState::Running),
        "claiming should move the run to running"
    );

    store
        .finalize_run(
            run.id,
            JobRunState::Succeeded,
            RunOutcome {
                status: Some(ResultStatus::Success),
                exit_code: Some(0),
                result: Some("out".into()),
                result_media_type: Some("text/plain".into()),
                ..Default::default()
            },
        )
        .await
        .expect("finalize_run");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let updated = runs.iter().find(|r| r.id == run.id).expect("run present");
    assert!(matches!(updated.state, JobRunState::Succeeded));
    assert_eq!(updated.exit_code, Some(0));
}

async fn listing_recent(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    for i in 0..3 {
        let ts = Utc::now() - Duration::seconds((i + 1) as i64);
        store
            .insert_job_run_if_missing(job, ts)
            .await
            .expect("insert run");
    }

    let all = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(all.len(), 3);
    for w in all.windows(2) {
        assert!(
            w[0].scheduled_for >= w[1].scheduled_for,
            "runs should be newest-first"
        );
    }

    let limited = store
        .list_recent_runs(Some(2), None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(limited.len(), 2, "limit should cap the result count");
}

async fn listing_filter_by_worker(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    for i in 0..3 {
        store
            .insert_job_run_if_missing(job, Utc::now() - Duration::seconds((i + 1) as i64))
            .await
            .expect("insert run");
    }
    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 2)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 2);

    let by_worker = store
        .list_recent_runs(None, None, None, None, Some(worker))
        .await
        .expect("list_recent_runs");
    assert_eq!(by_worker.len(), 2, "should list only this worker's runs");
    assert!(by_worker.iter().all(|r| r.worker_id == Some(worker)));
}

async fn listing_before_cursor(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    for i in 0..3 {
        store
            .insert_job_run_if_missing(job, Utc::now() - Duration::seconds((i + 1) as i64))
            .await
            .expect("insert run");
    }

    let before_future = store
        .list_recent_runs(
            Some(100),
            Some(Utc::now() + Duration::seconds(3600)),
            None,
            Some(job),
            None,
        )
        .await
        .expect("list_recent_runs");
    assert_eq!(
        before_future.len(),
        3,
        "before=future should return all runs"
    );

    let before_past = store
        .list_recent_runs(
            Some(100),
            Some(Utc::now() - Duration::seconds(3600)),
            None,
            Some(job),
            None,
        )
        .await
        .expect("list_recent_runs");
    assert!(
        before_past.is_empty(),
        "before=past should exclude newer runs"
    );
}

async fn claim_respects_limit(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    for i in 0..10 {
        store
            .insert_job_run_if_missing(job, Utc::now() - Duration::seconds((i + 1) as i64))
            .await
            .expect("insert run");
    }
    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 3)
        .await
        .expect("claim_job_runs");
    assert_eq!(
        claimed.len(),
        3,
        "claim must not exceed the requested limit"
    );
}

async fn claim_sets_worker_and_running(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    assert_eq!(
        claimed[0].worker_id,
        Some(worker),
        "claim must stamp the owner"
    );
    assert!(matches!(claimed[0].state, JobRunState::Running));
}

async fn claim_skips_disabled(store: StoreRef) {
    // The enabled gate lives at claim time, not at insert: a run can exist for a
    // job that is later disabled, but it must not be handed to a worker.
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    store.disable_job(job).await.expect("disable_job");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 10)
        .await
        .expect("claim_job_runs");
    assert!(
        claimed.is_empty(),
        "runs of a disabled job must not be claimed"
    );
}

async fn reaper_requeues_dead(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(30))
        .await
        .expect("insert run");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);

    // The worker goes silent: backdate its heartbeat instead of waiting.
    set_last_seen(&store, worker, Utc::now() - Duration::seconds(3600)).await;
    let requeued = store
        .reclaim_dead_workers_jobs(1)
        .await
        .expect("reclaim_dead_workers_jobs");
    assert_eq!(
        requeued, 1,
        "a dead worker's running run should be requeued"
    );

    // And it must be claimable again by a live worker.
    let worker2 = seed_worker(&store).await;
    let again = store
        .claim_job_runs(worker2, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(again.len(), 1, "requeued run should be claimable again");
}

async fn reaper_spares_live(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(30))
        .await
        .expect("insert run");

    let worker = seed_worker(&store).await; // last_seen = now (fresh)
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);

    let requeued = store
        .reclaim_dead_workers_jobs(3600)
        .await
        .expect("reclaim_dead_workers_jobs");
    assert_eq!(requeued, 0, "a live worker's run must not be reclaimed");
}

async fn state_cancel_prevents_claim(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(runs.len(), 1);
    store.cancel_run(runs[0].id).await.expect("cancel_run");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 10)
        .await
        .expect("claim_job_runs");
    assert!(claimed.is_empty(), "a cancelled run must not be claimable");
}

async fn state_adhoc_claimable(store: StoreRef) {
    let job = seed_job(&store, None, true).await;
    let run = store.create_adhoc_run(job).await.expect("create_adhoc_run");
    assert!(matches!(run.state, JobRunState::Queued));

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 10)
        .await
        .expect("claim_job_runs");
    assert!(
        claimed.iter().any(|r| r.id == run.id),
        "an ad-hoc run should be claimable"
    );
}

async fn claim_skips_deleted(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    store.delete_job(job).await.expect("delete_job");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 10)
        .await
        .expect("claim_job_runs");
    assert!(
        claimed.is_empty(),
        "runs of a soft-deleted job must not be claimed"
    );
}

async fn claim_orders_oldest_first(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let old = Utc::now() - Duration::seconds(100);
    let newer = Utc::now() - Duration::seconds(10);
    store
        .insert_job_run_if_missing(job, newer)
        .await
        .expect("insert newer");
    store
        .insert_job_run_if_missing(job, old)
        .await
        .expect("insert old");

    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    assert!(
        claimed[0].scheduled_for < Utc::now() - Duration::seconds(50),
        "claim should take the oldest due run first"
    );
}

async fn reaper_idempotent(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(30))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    set_last_seen(&store, worker, Utc::now() - Duration::seconds(3600)).await;

    assert_eq!(
        store
            .reclaim_dead_workers_jobs(1)
            .await
            .expect("reclaim_dead_workers_jobs"),
        1
    );
    assert_eq!(
        store
            .reclaim_dead_workers_jobs(1)
            .await
            .expect("reclaim_dead_workers_jobs"),
        0,
        "a second reclaim should be a no-op"
    );
}

async fn listing_after_cursor(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    for i in 0..3 {
        store
            .insert_job_run_if_missing(job, Utc::now() - Duration::seconds((i + 1) as i64))
            .await
            .expect("insert run");
    }

    let after_past = store
        .list_recent_runs(
            Some(100),
            None,
            Some(Utc::now() - Duration::seconds(3600)),
            Some(job),
            None,
        )
        .await
        .expect("list_recent_runs");
    assert_eq!(after_past.len(), 3, "after=past should return all runs");

    let after_future = store
        .list_recent_runs(
            Some(100),
            None,
            Some(Utc::now() + Duration::seconds(3600)),
            Some(job),
            None,
        )
        .await
        .expect("list_recent_runs");
    assert!(
        after_future.is_empty(),
        "after=future should exclude older runs"
    );
}

async fn worker_lookup_after_insert(store: StoreRef) {
    let id = Uuid::new_v4();
    store
        .insert_worker(id, "alice", "host-1", "v1", 0)
        .await
        .expect("insert_worker");
    let found = store.lookup_by_id(id).await.expect("lookup_by_id");
    let (name, _capacity) = found.expect("worker should be found by id");
    assert_eq!(name, "alice");
    assert!(
        store
            .lookup_by_id(Uuid::new_v4())
            .await
            .expect("lookup_by_id")
            .is_none(),
        "an unknown id returns None"
    );
}

async fn worker_incr_restart_count(store: StoreRef) {
    let id = Uuid::new_v4();
    store
        .insert_worker(id, "bob", "host-1", "v1", 0)
        .await
        .expect("insert_worker");
    let first = store
        .incr_restart_count(id, "v2")
        .await
        .expect("incr_restart_count");
    let second = store
        .incr_restart_count(id, "v3")
        .await
        .expect("incr_restart_count");
    assert_eq!(
        second,
        first + 1,
        "restart count should increment by one each call"
    );
}

async fn worker_list_registered(store: StoreRef) {
    store
        .insert_worker(Uuid::new_v4(), "w1", "host-1", "v1", 0)
        .await
        .expect("insert_worker");
    store
        .insert_worker(Uuid::new_v4(), "w2", "host-2", "v1", 0)
        .await
        .expect("insert_worker");
    let workers = store.list_workers().await.expect("list_workers");
    assert_eq!(workers.len(), 2, "both registered workers should be listed");
}

async fn state_failed_records_exit_and_error(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store
        .claim_job_runs(worker, 1)
        .await
        .expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);

    store
        .finalize_run(
            claimed[0].id,
            JobRunState::Failed,
            RunOutcome {
                status: Some(ResultStatus::Failed),
                exit_code: Some(1),
                error: Some("boom".to_string()),
                error_media_type: Some("text/plain".into()),
                ..Default::default()
            },
        )
        .await
        .expect("finalize_run");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let run = runs
        .iter()
        .find(|r| r.id == claimed[0].id)
        .expect("run present");
    assert!(matches!(run.state, JobRunState::Failed));
    assert_eq!(run.exit_code, Some(1));
    assert_eq!(run.error.as_deref(), Some("boom"));
}

async fn claim_claimed_run_not_double_claimed(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let w1 = seed_worker(&store).await;
    let first = store.claim_job_runs(w1, 10).await.expect("claim_job_runs");
    assert_eq!(first.len(), 1);

    let w2 = seed_worker(&store).await;
    let second = store.claim_job_runs(w2, 10).await.expect("claim_job_runs");
    assert!(
        second.is_empty(),
        "an already-running run must not be claimed again"
    );
}

async fn reaper_ignores_queued_runs(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    // A dead worker exists, but the run was never claimed (still queued).
    let worker = seed_worker(&store).await;
    set_last_seen(&store, worker, Utc::now() - Duration::seconds(3600)).await;

    let requeued = store
        .reclaim_dead_workers_jobs(1)
        .await
        .expect("reclaim_dead_workers_jobs");
    assert_eq!(
        requeued, 0,
        "reaper must only touch running runs, not queued ones"
    );
}

async fn mat_distinct_jobs_independent(store: StoreRef) {
    let job_a = seed_job(&store, Some("* * * * *"), true).await;
    let job_b = seed_job(&store, Some("* * * * *"), true).await;
    let ts = Utc::now() - Duration::seconds(10);

    assert!(
        store
            .insert_job_run_if_missing(job_a, ts)
            .await
            .expect("insert a")
    );
    assert!(
        store
            .insert_job_run_if_missing(job_b, ts)
            .await
            .expect("insert b"),
        "the same timestamp on a different job is a distinct run"
    );
    assert_eq!(
        store
            .list_recent_runs(None, None, None, Some(job_a), None)
            .await
            .expect("list a")
            .len(),
        1
    );
    assert_eq!(
        store
            .list_recent_runs(None, None, None, Some(job_b), None)
            .await
            .expect("list b")
            .len(),
        1
    );
}

async fn crud_update_job(store: StoreRef) {
    let job = store
        .create_job(
            "orig",
            Some("0 0 * * *".to_string()),
            shell(),
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");

    store
        .update_job(
            job.id,
            Some("renamed".to_string()),
            Some(Some("* * * * *".to_string())),
            None,
            Some(5),
            None,
            None,
        )
        .await
        .expect("update_job");

    let got = store.get_job(job.id).await.expect("get_job");
    assert_eq!(got.name, "renamed");
    assert_eq!(got.max_concurrency, 5);
    assert_eq!(got.schedule_cron.as_deref(), Some("* * * * *"));
}

async fn crud_update_user(store: StoreRef) {
    let user = store
        .create_user("carol", "h1", UserRole::Viewer, None)
        .await
        .expect("create_user");

    store
        .update_user(user.id, None, Some("h2"), Some(UserRole::Admin))
        .await
        .expect("update_user");

    let got = store.get_user_by_id(user.id).await.expect("get_user_by_id");
    assert_eq!(got.username, "carol", "username should be unchanged when None");
    assert!(matches!(got.role, UserRole::Admin), "role should be updated");
}

async fn retention_prunes_old_terminal(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let old = Utc::now() - Duration::days(30);
    let recent = Utc::now() - Duration::seconds(10);
    store.insert_job_run_if_missing(job, old).await.expect("insert old");
    store
        .insert_job_run_if_missing(job, recent)
        .await
        .expect("insert recent");

    // Both runs reach a terminal state.
    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    for r in &runs {
        store
            .finalize_run(
                r.id,
                JobRunState::Succeeded,
                RunOutcome {
                    status: Some(ResultStatus::Success),
                    exit_code: Some(0),
                    ..Default::default()
                },
            )
            .await
            .expect("finalize_run");
    }

    let deleted = store
        .prune_runs(Utc::now() - Duration::days(1))
        .await
        .expect("prune_runs");
    assert_eq!(deleted, 1, "only the old terminal run should be pruned");

    let remaining = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    assert_eq!(remaining.len(), 1);
    assert!(
        remaining[0].scheduled_for > Utc::now() - Duration::days(1),
        "the recent run should survive"
    );
}

async fn retention_spares_active(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    // An old run that is still queued (active), never completed.
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::days(30))
        .await
        .expect("insert run");

    let deleted = store
        .prune_runs(Utc::now() - Duration::days(1))
        .await
        .expect("prune_runs");
    assert_eq!(
        deleted, 0,
        "active (non-terminal) runs must not be pruned, even when old"
    );
    assert_eq!(
        store
            .list_recent_runs(None, None, None, Some(job), None)
            .await
            .expect("list_recent_runs")
            .len(),
        1
    );
}

async fn settings_set_get(store: StoreRef) {
    assert!(
        store
            .get_setting("scheduler.misfire_catchup_secs")
            .await
            .expect("get_setting")
            .is_none(),
        "unknown key returns None"
    );
    store
        .set_setting("scheduler.misfire_catchup_secs", "600")
        .await
        .expect("set_setting");
    assert_eq!(
        store
            .get_setting("scheduler.misfire_catchup_secs")
            .await
            .expect("get_setting"),
        Some("600".to_string())
    );
}

async fn settings_overwrite(store: StoreRef) {
    store.set_setting("k", "1").await.expect("set_setting");
    store.set_setting("k", "2").await.expect("set_setting");
    assert_eq!(
        store.get_setting("k").await.expect("get_setting"),
        Some("2".to_string()),
        "set overwrites the previous value"
    );
}

async fn settings_list(store: StoreRef) {
    store.set_setting("a", "1").await.expect("set_setting");
    store.set_setting("b", "2").await.expect("set_setting");
    let all = store.list_settings().await.expect("list_settings");
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|s| s.key == "a" && s.value == "1"));
    assert!(all.iter().any(|s| s.key == "b" && s.value == "2"));
}

// Enforces that a claim returns a usable config snapshot -- the gap that let SQLite
// pass every other case while being unable to execute anything.
async fn claim_carries_shell_snapshot(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let snap = claimed[0]
        .snapshot
        .as_ref()
        .expect("claim must return a usable config snapshot");
    match &snap.meta {
        ExecutableConfigSnapshotMeta::Shell { command, .. } => assert_eq!(command, "echo hi"),
        other => panic!("expected a Shell snapshot, got {}", other.type_of_str()),
    }
}

async fn claim_carries_http_snapshot(store: StoreRef) {
    let job = store
        .create_job(
            "http-job",
            Some("* * * * *".to_string()),
            RunnerConfig::Http {
                method: "POST".to_string(),
                url: "http://example.test/hook".to_string(),
                headers: None,
                body: Some("hi".to_string()),
                timeout_sec: Some(3),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let snap = claimed[0]
        .snapshot
        .as_ref()
        .expect("claim must return a usable config snapshot");
    match &snap.meta {
        ExecutableConfigSnapshotMeta::Http {
            method, url, body, ..
        } => {
            assert_eq!(method, "POST");
            assert!(url.contains("example.test"));
            assert_eq!(body.as_deref(), Some("hi"));
        }
        other => panic!("expected an Http snapshot, got {}", other.type_of_str()),
    }
}

async fn claim_carries_python_snapshot(store: StoreRef) {
    let job = store
        .create_job(
            "py-job",
            Some("* * * * *".to_string()),
            RunnerConfig::Python {
                module: "mymod".to_string(),
                class_name: "MyTask".to_string(),
                timeout_sec: Some(10),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let snap = claimed[0]
        .snapshot
        .as_ref()
        .expect("claim must return a usable config snapshot");
    match &snap.meta {
        ExecutableConfigSnapshotMeta::Python {
            module, class_name, ..
        } => {
            assert_eq!(module, "mymod");
            assert_eq!(class_name, "MyTask");
        }
        other => panic!("expected a Python snapshot, got {}", other.type_of_str()),
    }
}

async fn claim_carries_node_snapshot(store: StoreRef) {
    let job = store
        .create_job(
            "node-job",
            Some("* * * * *".to_string()),
            RunnerConfig::Node {
                module: "mymod".to_string(),
                function_name: "run".to_string(),
                timeout_sec: Some(10),
            },
            1,
            MisfirePolicy::RunImmediately,
            RetryConfig::default(),
        )
        .await
        .expect("create_job");
    store.enable_job(job.id).await.expect("enable_job");
    store
        .insert_job_run_if_missing(job.id, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let snap = claimed[0]
        .snapshot
        .as_ref()
        .expect("claim must return a usable config snapshot");
    match &snap.meta {
        ExecutableConfigSnapshotMeta::Node {
            module,
            function_name,
            ..
        } => {
            assert_eq!(module, "mymod");
            assert_eq!(function_name, "run");
        }
        other => panic!("expected a Node snapshot, got {}", other.type_of_str()),
    }
}

async fn env_set_get_roundtrip(store: StoreRef) {
    let job = seed_job(&store, None, false).await;
    assert!(
        store.get_job_env(job).await.expect("get_job_env").is_empty(),
        "a fresh job has no env"
    );

    let mut env = HashMap::new();
    env.insert("FOO".to_string(), "bar".to_string());
    env.insert("BAZ".to_string(), "qux".to_string());
    store.set_job_env(job, env).await.expect("set_job_env");

    let got = store.get_job_env(job).await.expect("get_job_env");
    assert_eq!(got.len(), 2);
    assert_eq!(got.get("FOO").map(String::as_str), Some("bar"));
    assert_eq!(got.get("BAZ").map(String::as_str), Some("qux"));
}

async fn env_set_replaces_all(store: StoreRef) {
    let job = seed_job(&store, None, false).await;

    let mut first = HashMap::new();
    first.insert("A".to_string(), "1".to_string());
    store.set_job_env(job, first).await.expect("set_job_env");

    let mut second = HashMap::new();
    second.insert("B".to_string(), "2".to_string());
    store.set_job_env(job, second).await.expect("set_job_env");

    let got = store.get_job_env(job).await.expect("get_job_env");
    assert_eq!(got.len(), 1, "set should replace the full env, not merge");
    assert!(got.contains_key("B") && !got.contains_key("A"));
}

// Enforces that env set on a job reaches the claimed run's snapshot (both backends),
// so subprocess runners actually receive PYTHONPATH/NODE_PATH/etc.
async fn claim_carries_env_snapshot(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    let mut env = HashMap::new();
    env.insert("FOO".to_string(), "bar".to_string());
    store.set_job_env(job, env).await.expect("set_job_env");
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    let snap = claimed[0]
        .snapshot
        .as_ref()
        .expect("claim must return a usable config snapshot");
    match &snap.meta {
        ExecutableConfigSnapshotMeta::Shell { env, .. } => {
            assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
        }
        other => panic!("expected a Shell snapshot, got {}", other.type_of_str()),
    }
}

async fn outcome_records_result(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);

    store
        .finalize_run(
            claimed[0].id,
            JobRunState::Succeeded,
            RunOutcome {
                status: Some(ResultStatus::Success),
                exit_code: Some(0),
                stdout: Some("log line".into()),
                result: Some("{\"rows\":42}".into()),
                result_media_type: Some("application/json".into()),
                ..Default::default()
            },
        )
        .await
        .expect("finalize_run");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let r = runs.iter().find(|r| r.id == claimed[0].id).expect("present");
    assert!(matches!(r.state, JobRunState::Succeeded));
    assert!(matches!(r.result_status, Some(ResultStatus::Success)));
    assert_eq!(r.result.as_deref(), Some("{\"rows\":42}"));
    assert_eq!(r.result_media_type.as_deref(), Some("application/json"));
    assert_eq!(r.stdout.as_deref(), Some("log line"));
    assert!(r.finished_at.is_some());
}

async fn outcome_records_error(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");

    store
        .finalize_run(
            claimed[0].id,
            JobRunState::Failed,
            RunOutcome {
                status: Some(ResultStatus::Failed),
                exit_code: Some(1),
                error: Some("{\"type\":\"ValueError\",\"message\":\"boom\"}".into()),
                error_media_type: Some("application/json".into()),
                ..Default::default()
            },
        )
        .await
        .expect("finalize_run");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let r = runs.iter().find(|r| r.id == claimed[0].id).expect("present");
    assert!(matches!(r.result_status, Some(ResultStatus::Failed)));
    assert!(r.error.as_deref().unwrap_or_default().contains("boom"));
    assert_eq!(r.error_media_type.as_deref(), Some("application/json"));
}

async fn retry_reschedule_requeues(store: StoreRef) {
    let job = seed_job(&store, Some("* * * * *"), true).await;
    store
        .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
        .await
        .expect("insert run");
    let worker = seed_worker(&store).await;
    let claimed = store.claim_job_runs(worker, 1).await.expect("claim_job_runs");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempt, 1, "first claim is attempt 1");

    let future = Utc::now() + Duration::seconds(3600);
    store
        .reschedule_for_retry(
            claimed[0].id,
            2,
            future,
            RunOutcome {
                status: Some(ResultStatus::Retryable),
                error: Some("transient".into()),
                error_media_type: Some("text/plain".into()),
                ..Default::default()
            },
        )
        .await
        .expect("reschedule_for_retry");

    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let r = runs.iter().find(|r| r.id == claimed[0].id).expect("present");
    assert!(
        matches!(r.state, JobRunState::Queued),
        "a rescheduled run returns to queued"
    );
    assert_eq!(r.attempt, 2, "attempt is bumped");
    assert!(r.worker_id.is_none(), "owner is cleared on requeue");

    // Scheduled in the future, so it is not yet claimable.
    let w2 = seed_worker(&store).await;
    let again = store.claim_job_runs(w2, 10).await.expect("claim_job_runs");
    assert!(
        again.is_empty(),
        "a future-scheduled retry is not claimable yet"
    );
}

async fn secrets_roundtrip(store: StoreRef) {
    let id = store
        .upsert_secret(DEFAULT_TENANT_ID, "db-pass", b"CT", b"NONCE", "xchacha20poly1305", b"WRAPPED", 1)
        .await
        .expect("upsert_secret");

    let by_name = store
        .get_secret_by_name(DEFAULT_TENANT_ID, "db-pass")
        .await
        .expect("get_secret_by_name")
        .expect("present");
    assert_eq!(by_name.id, id);
    assert_eq!(by_name.value_ct, b"CT".to_vec());
    assert_eq!(by_name.value_nonce, b"NONCE".to_vec());
    assert_eq!(by_name.dek_wrapped, b"WRAPPED".to_vec());
    assert_eq!(by_name.aead_algo, "xchacha20poly1305");
    assert_eq!(by_name.kek_version, 1);

    let by_id = store.get_secret(id, None).await.expect("get_secret").expect("present");
    assert_eq!(by_id.name, "db-pass");

    // Listing exposes metadata only; SecretMeta has no ciphertext field.
    let names = store.list_secret_names(None).await.expect("list_secret_names");
    assert!(names.iter().any(|m| m.name == "db-pass" && m.id == id));
}

async fn secrets_replace(store: StoreRef) {
    let id1 = store
        .upsert_secret(DEFAULT_TENANT_ID, "k", b"v1", b"n1", "a", b"w1", 1)
        .await
        .expect("upsert");
    let id2 = store
        .upsert_secret(DEFAULT_TENANT_ID, "k", b"v2", b"n2", "a", b"w2", 2)
        .await
        .expect("upsert");
    assert_eq!(id1, id2, "upsert by name keeps the same id");

    let s = store
        .get_secret_by_name(DEFAULT_TENANT_ID, "k")
        .await
        .expect("get")
        .expect("present");
    assert_eq!(s.value_ct, b"v2".to_vec());
    assert_eq!(s.kek_version, 2);
}

async fn secrets_delete(store: StoreRef) {
    let id = store
        .upsert_secret(DEFAULT_TENANT_ID, "d", b"x", b"n", "a", b"w", 1)
        .await
        .expect("upsert");
    store.delete_secret(id).await.expect("delete_secret");
    assert!(store.get_secret(id, None).await.expect("get").is_none());
}

async fn secrets_kek_roundtrip(store: StoreRef) {
    store
        .insert_kek_version(1, "active")
        .await
        .expect("insert_kek_version");
    let versions = store.list_kek_versions().await.expect("list_kek_versions");
    assert!(versions.iter().any(|v| v.version == 1 && v.state == "active"));

    let node = Uuid::new_v4();
    store
        .put_kek_share(1, node, b"SEALED")
        .await
        .expect("put_kek_share");
    let share = store
        .get_kek_share(1, node)
        .await
        .expect("get_kek_share")
        .expect("present");
    assert_eq!(share.wrapped_kek, b"SEALED".to_vec());
    assert!(share.acked_at.is_none(), "fresh share is not acked");
}

async fn secrets_node_key(store: StoreRef) {
    let node = Uuid::new_v4();
    store
        .upsert_node_key(node, 1, b"PUBKEY", "approved")
        .await
        .expect("upsert_node_key");
    let keys = store.list_node_keys().await.expect("list_node_keys");
    let k = keys
        .iter()
        .find(|k| k.node_id == node)
        .expect("node key present");
    assert_eq!(k.public_key, b"PUBKEY".to_vec());
    assert_eq!(k.status, "approved");
    assert_eq!(k.key_version, 1);
}

async fn secrets_tenant_isolation(store: StoreRef) {
    let other = store
        .create_tenant("other-co")
        .await
        .expect("create_tenant")
        .id;
    let id = store
        .upsert_secret(DEFAULT_TENANT_ID, "api-key", b"ct", b"n", "a", b"w", 1)
        .await
        .expect("upsert");

    assert!(
        store
            .get_secret_by_name(other, "api-key")
            .await
            .expect("get")
            .is_none(),
        "another tenant cannot resolve it by name"
    );
    assert!(
        store
            .get_secret(id, Some(other))
            .await
            .expect("get")
            .is_none(),
        "another tenant cannot read it by id"
    );
    assert!(
        store
            .get_secret(id, Some(DEFAULT_TENANT_ID))
            .await
            .expect("get")
            .is_some(),
        "the owning tenant can read it"
    );
    let names = store
        .list_secret_names(Some(other))
        .await
        .expect("list_secret_names");
    assert!(!names.iter().any(|m| m.name == "api-key"));

    // The same name in a different tenant is a distinct secret (per-tenant uniqueness).
    let id2 = store
        .upsert_secret(other, "api-key", b"ct2", b"n2", "a", b"w2", 1)
        .await
        .expect("upsert other");
    assert_ne!(id, id2);
}

async fn tenant_create_get_list(store: StoreRef) {
    let created = store.create_tenant("acme").await.expect("create_tenant");
    assert_eq!(created.name, "acme");

    let got = store
        .get_tenant(created.id)
        .await
        .expect("get_tenant")
        .expect("present");
    assert_eq!(got.id, created.id);
    assert_eq!(got.name, "acme");

    let all = store.list_tenants().await.expect("list_tenants");
    assert!(all.iter().any(|t| t.name == "acme"));
    assert!(
        all.iter().any(|t| t.name == "default"),
        "the seeded default tenant is always present"
    );
}

async fn tenant_user_scope(store: StoreRef) {
    let tenant = store.create_tenant("scoped").await.expect("create_tenant");

    let system = store
        .create_user("sysadmin", "h", UserRole::Admin, None)
        .await
        .expect("create_user");
    assert!(system.tenant_id.is_none(), "a system user has no tenant");

    let scoped = store
        .create_user("tenantadmin", "h", UserRole::Admin, Some(tenant.id))
        .await
        .expect("create_user");
    assert_eq!(scoped.tenant_id, Some(tenant.id));

    let fetched = store
        .get_user_by_username("tenantadmin")
        .await
        .expect("get_user_by_username");
    assert_eq!(
        fetched.tenant_id,
        Some(tenant.id),
        "tenant scope survives a read"
    );
}

async fn durability_definitions_survive(handle: Box<dyn DurableHandle>) {
    let job_id = {
        let store = handle.open().await;
        let job = store
            .create_job(
                "persistent",
                Some("* * * * *".to_string()),
                shell(),
                2,
                MisfirePolicy::RunImmediately,
                RetryConfig::default(),
            )
            .await
            .expect("create_job");
        store.enable_job(job.id).await.expect("enable_job");
        job.id
    }; // first handle dropped -> simulated restart

    let store = handle.open().await;
    let got = store
        .get_job(job_id)
        .await
        .expect("job definition should survive a reopen");
    assert_eq!(got.name, "persistent");
    assert!(got.enabled, "enabled flag should survive a reopen");
    assert_eq!(got.max_concurrency, 2);
}

async fn durability_inflight_run_recoverable(handle: Box<dyn DurableHandle>) {
    let (job, run_id, worker) = {
        let store = handle.open().await;
        let job = seed_job(&store, Some("* * * * *"), true).await;
        store
            .insert_job_run_if_missing(job, Utc::now() - Duration::seconds(10))
            .await
            .expect("insert run");
        let worker = seed_worker(&store).await;
        let claimed = store
            .claim_job_runs(worker, 1)
            .await
            .expect("claim_job_runs");
        assert_eq!(claimed.len(), 1);
        (job, claimed[0].id, worker)
    }; // restart while the run is in flight

    let store = handle.open().await;
    let runs = store
        .list_recent_runs(None, None, None, Some(job), None)
        .await
        .expect("list_recent_runs");
    let run = runs
        .iter()
        .find(|r| r.id == run_id)
        .expect("in-flight run should survive a reopen");
    assert!(
        matches!(run.state, JobRunState::Running),
        "the claimed run persists as running across a reopen"
    );

    // The owner did not come back: the reaper recovers the run after the restart.
    set_last_seen(&store, worker, Utc::now() - Duration::seconds(3600)).await;
    let requeued = store
        .reclaim_dead_workers_jobs(1)
        .await
        .expect("reclaim_dead_workers_jobs");
    assert_eq!(
        requeued, 1,
        "an in-flight run is recoverable after a restart"
    );
}

async fn leadership_single_leader(pair: (StoreRef, StoreRef)) {
    let (a, b) = pair;
    // Two nodes against one backend: at most one may consider itself leader, so a
    // single scheduler materializes. First caller wins; the other is fenced out.
    let a_leader = a.am_i_leader().await.expect("am_i_leader");
    let b_leader = b.am_i_leader().await.expect("am_i_leader");
    let leaders = [a_leader, b_leader].into_iter().filter(|x| *x).count();
    assert_eq!(leaders, 1, "exactly one of two nodes should win leadership");
}

async fn leadership_stable_across_calls(pair: (StoreRef, StoreRef)) {
    let (a, _b) = pair;
    // A node that won leadership must stay leader across repeated calls (renew),
    // never flip -- this catches a pooled/session-scoped lock routing inconsistently.
    for _ in 0..5 {
        assert!(
            a.am_i_leader().await.expect("am_i_leader"),
            "a node that won leadership should stay leader across repeated calls"
        );
    }
}
