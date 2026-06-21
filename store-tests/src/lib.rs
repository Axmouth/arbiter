//! Shared, backend-agnostic conformance suite for the arbiter `Store` trait.
//!
//! The suite is a registry of [`Case`]s (data); a runner crosses every backend
//! with every applicable case. The runner and the per-backend [`BackendFactory`]
//! implementations live in `tests/conformance.rs` (that is where, for example,
//! Postgres is set up). Nothing here knows about a concrete backend.

use std::collections::HashSet;
use std::sync::Arc;

// `Store` brings its supertrait methods (ApiStore/JobStore/RunStore/WorkerStore)
// into scope for `dyn Store`, so only the trait and the data types are imported.
use arbiter_core::{JobRunState, MisfirePolicy, RunnerConfig, Store, UserRole};
use chrono::{Duration, Utc};
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

/// A backend the suite can grade. Implemented once per store, in the test wiring,
/// where it also owns its resource lifecycle (container, temp dir, ...).
#[async_trait::async_trait]
pub trait BackendFactory: Send + Sync {
    fn name(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    /// A fresh, empty, isolated store.
    async fn fresh(&self) -> StoreRef;

    /// Reopen the same durable store after a simulated restart (durability group).
    async fn reopen(&self, _store: StoreRef) -> Option<StoreRef> {
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

// --- cases ---

async fn crud_job_create_get(store: StoreRef) {
    let job = store
        .create_job("alpha", None, shell(), 3, MisfirePolicy::RunImmediately)
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
        .create_user("admin", "hash", UserRole::Admin)
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
        .update_job_run_state(run.id, JobRunState::Succeeded, Some(0), Some("out".into()), None)
        .await
        .expect("update_job_run_state");

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
