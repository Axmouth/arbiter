# Arbiter Backend Conformance Suite

Status: design exploration (no code committed from this doc). Companion to
`BACKEND_ARCHITECTURE.md`.

Goal: **one test suite, many backends.** Write the behavioral contract of a Store
backend once, validate Postgres against it today, and run the *same* suite against
every future backend (SQLite, Mongo, Ganglion-coordinated, distributed SQLite, the
self-made Keratin store). A new backend is "done" when it passes the suite at the
capability level it claims.

This is the same pattern Ganglion already relies on: openraft ships
`openraft::testing::Suite::test_all(builder)` and Ganglion runs it against both its
log stores. We want Arbiter's equivalent for `Store`.

---

## 1. How to implement it

### 1.1 The core idea: a shared suite of cases + a backend factory

The suite never names a concrete backend. It is two things in the `arbiter-store-tests`
library (which depends only on `arbiter-core`):

- a **registry of cases** -- the behavioral contract expressed as *data*, so a runner
  can cross every backend with every applicable case:

  ```rust
  pub type StoreRef = std::sync::Arc<dyn arbiter_core::Store + Send + Sync>;

  pub struct Case {
      pub group: &'static str,
      pub name:  &'static str,
      pub needs: &'static [Capability],          // capabilities this case requires
      pub run:   fn(StoreRef) -> BoxFuture<'static, ()>,
  }
  pub fn cases() -> Vec<Case> { /* every group, as data */ }
  ```

- a **backend factory** each backend implements once, producing a fresh isolated store
  on demand and declaring its capabilities:

  ```rust
  #[async_trait]
  pub trait BackendFactory: Send + Sync {
      fn name(&self) -> &'static str;
      fn capabilities(&self) -> Capabilities;
      async fn fresh(&self) -> StoreRef;                       // isolated, empty
      async fn reopen(&self, s: StoreRef) -> Option<StoreRef> { None } // durability
      // cluster(n) added later, for the HA group
  }
  ```

  It returns `dyn Store` rather than an associated type so factories can live in a
  `Vec<Box<dyn BackendFactory>>` the runner iterates.

Isolation per `fresh()` is essential: a temp file for SQLite/Keratin; a uniquely-named
database or schema for Postgres/Mongo. Tests must never share state.

### 1.2 Capability flags keep it honest

Not every backend supports every behavior, and pretending otherwise produces either
false failures or skipped-and-forgotten gaps. Declare capabilities explicitly, and
have the suite *record* skips with a reason rather than silently passing.

The key insight for making the suite seamless across single-node and clustered
backends: **"clustering" is not one lever -- it is two orthogonal axes.** Conflating
them would wrongly skip tests that single-node backends can and should pass.

- **multi_worker** -- more than one worker identity claims/contends against the *same*
  backend instance. This is about *concurrency correctness*, not topology. Single-node
  SQLite has this (multiple processes/threads on one host share the file; writes
  serialize but each run is still claimed exactly once). Postgres has it. It gates
  claim concurrency, leader election, and the reaper.
- **multi_node** -- more than one backend *instance*, with replication and failover.
  This is real clustering. Single-node SQLite does NOT have it; distributed SQLite and
  Ganglion-coordinated do; Postgres does externally. It gates replication visibility,
  failover, and cross-node fencing.

```rust
pub struct Capabilities {
    pub durable: bool,         // survives reopen()
    pub native_unique: bool,   // DB-enforced uniqueness vs in-memory index
    pub multi_worker: bool,    // N claimers vs one backend instance (correctness)
    pub leader_election: bool, // exclusive "am I the scheduler" among workers
    pub multi_node: bool,      // N backend instances: replication + failover
    pub fencing: bool,         // epoch-conditional writes rejected when stale
    pub retention: bool,       // truncate/compact old runs
}
```

Worked profiles:

| Backend | durable | multi_worker | leader_election | multi_node | fencing | retention |
| --- | --- | --- | --- | --- | --- | --- |
| In-memory (tests) | no | yes | no | no | no | no |
| Single-node SQLite | yes | yes | yes (lease row) | no | no | yes |
| Postgres | yes | yes | yes (advisory lock) | external | n/a | yes |
| Distributed SQLite | yes | yes | yes (Ganglion) | yes | yes | yes |
| Ganglion-coordinated | yes | yes | yes (raft) | yes | yes | yes |

Single-node SQLite is `multi_worker=true, multi_node=false`: it runs the
claim-concurrency, leadership, reaper, durability, and retention groups unchanged, and
only the replication/failover groups skip. That is the bulk of the suite -- the
"seamless for SQLite" property falls straight out of splitting the axis correctly.
(Leader election on single-node SQLite is a lease row, not an advisory lock or raft --
the suite tests the *contract*, "at most one scheduler materializes", not the
mechanism.)

The suite runs the intersection of (test requirements) and (declared capabilities)
and prints a coverage matrix.

### 1.3 The runner (libtest-mimic)

`tests/conformance.rs` with `harness = false` crosses backends x cases, one test per
combo, with capability gaps surfaced as `ignored`:

```rust
fn main() {
    let backends: Vec<Box<dyn BackendFactory>> =
        vec![Box::new(PgBackend::new()) /*, SqliteBackend, ... */];

    let mut trials = vec![];
    for b in backends {
        let b: std::sync::Arc<dyn BackendFactory> = b.into();
        for c in cases() {
            let name = format!("{}::{}::{}", b.name(), c.group, c.name);
            if b.capabilities().satisfies(c.needs) {
                let b = b.clone();
                trials.push(Trial::test(name, move || {
                    tokio::runtime::Runtime::new()?
                        .block_on(async { (c.run)(b.fresh().await).await });
                    Ok(())
                }));
            } else {
                trials.push(Trial::test(name, || Ok(())).with_ignored_flag(true));
            }
        }
    }
    libtest_mimic::run(&libtest_mimic::Arguments::from_args(), trials).exit();
}
```

`cargo test` reports `pg::crud::job_create_get ... ok`, `pg::ha::failover ... ignored`,
etc. -- per-combo, parallel, filterable (`cargo test pg::claim`). libtest-mimic catches
panics, so an `assert!` in a case becomes a clean failure.

### 1.4 Where PG is set up (and adding a backend)

Backends are wired in the conformance crate's `tests/`, never inside their own crates
-- that keeps docker out of `store-pg` and the suite backend-agnostic. The crate's
dev-dependencies pull in the concrete backends + `testcontainers` + `libtest-mimic`.

`PgBackend` is the single home for PG setup: on construction it reads
`ARBITER_TEST_DATABASE_URL`, or spins one Postgres container for the test process; each
`fresh()` creates a new database, applies the schema (`docker/init`), and returns a
`PgStore`. SQLite's factory hands out a fresh temp file. Adding a backend = implement
`BackendFactory` once and add one line to the runner's `Vec`.

### 1.5 CI wiring

- Backends with no external dependency (SQLite, self-made, in-memory) run on every
  CI job.
- Postgres/Mongo run in a service container (or are skipped when the env var is
  absent, so local `cargo test` still works without them).
- Distributed/HA suites are heavier; gate them behind a `--features ha-tests` or a
  separate CI job, like Ganglion keeps its slow cluster tests separate.

### 1.6 Bringing up single-node SQLite (worked approach)

Concrete recipe, since SQLite is the planned second backend:

- **Fixture:** `fresh()` opens a fresh temp-dir SQLite file in WAL mode; `reopen()`
  closes and reopens the same file (durability tests). For multi_worker tests, hand
  out N `Store` handles that are N connections to the *same* file.
- **Capabilities:** `durable + native_unique + multi_worker + leader_election +
  retention`; `multi_node=false`, `fencing=false`.
- **What runs:** CRUD, materialization, state machine, listing, claim-concurrency,
  leadership, reaper, durability, retention -- the whole single-cluster suite.
- **What skips (recorded):** replication, failover, cross-node fencing.
- **What the suite forces you to get right:** the claim-concurrency group is where
  SQLite is trickiest -- no `SKIP LOCKED`, one writer at a time -- so `claim_job_runs`
  must use a conditional `UPDATE ... WHERE state='queued' ... RETURNING` and retry on
  `SQLITE_BUSY`. The "each run claimed exactly once under N claimers" test is exactly
  the assertion that proves you did this right. Leader election needs a lease row
  (CAS on an expiry column) since there is no advisory lock.

In short: implement `SqliteStore` + a `SqliteCoordinator` (lease-row leader, last_seen
reaper), point the fixture at it, and the same suite that graded Postgres grades
SQLite -- with the matrix making the (expected) HA gaps explicit rather than hidden.

---

## 2. What it looks like in practice

Each case takes a fresh `StoreRef` (the runner calls `fresh()` and hands it in) and
reads as plain behavior against the trait, never the backend:

```rust
// group "materialization"
async fn insert_is_idempotent(store: StoreRef) {
    let job = seed_enabled_cron_job(&store, "* * * * *").await; // enabled: insert requires it
    let ts = dt("2026-01-01T00:00:00Z");

    assert!(store.insert_job_run_if_missing(job.id, ts).await.unwrap());  // inserted
    assert!(!store.insert_job_run_if_missing(job.id, ts).await.unwrap()); // existed
    assert_eq!(store.list_recent_runs(None,None,None,Some(job.id),None).await.unwrap().len(), 1);
}
```

A concurrency test uses real parallelism and asserts an invariant, not a sequence:

```rust
// group "claim"  (needs Capability::MultiWorker)
async fn each_run_claimed_once(store: StoreRef) {
    seed_due_runs(&store, 100).await;

    // N workers race to claim
    let claimed: Vec<JobRun> = join_all((0..8).map(|_| {
        let s = store.clone();
        let wid = Uuid::new_v4();
        async move { s.claim_job_runs(wid, 100).await.unwrap() }
    })).await.into_iter().flatten().collect();

    let ids: HashSet<_> = claimed.iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), claimed.len(), "a run was claimed by two workers");
    assert_eq!(claimed.len(), 100, "all due runs claimed exactly once");
}
```

An HA test drives the cluster fixture:

```rust
// ha::run  (requires caps.replicated)
async fn write_visible_after_failover<F: StoreFixture>(f: &F) {
    let mut c = f.cluster(3).await.unwrap();
    let job = seed_cron_job(c.primary(), "* * * * *").await;
    c.wait_replicated().await;
    c.kill_primary().await;          // ganglion promotes a standby, bumps epoch
    let new = c.wait_new_primary().await;
    assert!(new.get_job(job.id).await.is_ok(), "definition lost across failover");
}
```

---

## 3. Tests to focus on

Grouped by category, with the invariant each protects and the capability it needs.
Start at the top; the first four groups apply to every backend.

### 3.1 CRUD round-trips (always)

- Create/get/update/delete for jobs (incl. each runner config type), workers, users.
- Soft-delete (`jobs.deleted_at`) hides from `list_jobs`/`list_enabled_cron_jobs` but
  remains referenceable by historical runs.
- Foreign-key-ish integrity: a run references an existing job; deleting a job with
  runs behaves per the documented rule.
- Unique username enforced (native or in-memory).

### 3.2 Materialization (always)

- `insert_job_run_if_missing` returns `true` once, `false` thereafter for the same
  `(job_id, scheduled_for)`.
- Concurrent double-insert of the same `(job, ts)` yields exactly one row (the core
  exactly-once-materialization guarantee).
- Materialized run starts in `queued` with a `config_snapshot` present (the worker
  aborts without one, `worker/src/lib.rs:91`).

### 3.3 State machine (always)

- Valid transitions persist: `queued -> running -> succeeded|failed|cancelled`.
- Terminal states are immutable (no `succeeded -> running`).
- `cancel_run` on a queued run prevents it from being claimed; cancel semantics on a
  running run are defined and tested.
- `update_job_run_state` records exit_code/output/error_output correctly.

### 3.4 Listing / queries (always)

- `list_recent_runs` ordering is `scheduled_for DESC`.
- Cursor paging (`before`/`after`) is stable and non-overlapping across pages.
- Filters compose: `by_job_id`, `by_worker_id`, `limit`.
- Empty/edge results (no runs, limit 0) behave.

### 3.5 Claim concurrency (caps.multi_worker)

- Each due run is claimed by exactly one worker under N-way contention (the test in
  section 2).
- Only `queued AND scheduled_for <= now` runs are claimed (not future, not running).
- `limit`/capacity respected; a worker never claims more than asked.
- A claimed run flips to `running` with the claiming `worker_id` atomically (no
  visible half-claimed state).

### 3.6 Leadership (caps.leader_election)

- Across N nodes, at most one reports `am_i_leader() == true` at any instant.
- On leader loss, a new leader emerges within the expected window.
- Only the leader materializes (no duplicate materialization from two schedulers) —
  the end-to-end version of 3.2 across nodes.

### 3.7 Fencing (caps.fencing)

- A write/claim tagged with a stale epoch is rejected; current epoch accepted.
- After ownership changes (epoch bump), the old owner's stale-epoch writes are
  refused — the split-brain guard. This is the single most important distributed
  test; everything else is comparatively forgiving.

### 3.8 Reaper / liveness (caps.multi_worker)

- A stale worker's `running` runs are requeued to `queued` after `dead_after_secs`.
- A live worker's runs are left alone (no premature reaping — guard against
  clock-skew false positives).
- Reaper is idempotent (running it twice does not double-requeue or corrupt state).

### 3.9 Durability / crash recovery (caps.durable)

- Write data, `reopen()` the store, assert everything survives (definitions, runs,
  users).
- In-flight `running` runs are recoverable after restart (either resumed or requeued
  per the documented contract).
- No partial-write corruption: a kill mid-write leaves the store readable and
  consistent (for backends that can simulate it).

### 3.10 HA / replication (caps.multi_node)

- A write on the primary is visible on a standby after replication catches up.
- Failover promotes the most-caught-up standby; Tier-1 data (definitions, users,
  control records) survives the cut (section 2 test).
- Read-your-writes: a client that wrote via the primary does not observe a stale
  replica.
- Partition the primary mid-write, promote a standby, assert the old primary's
  stale-epoch writes are rejected (the Jepsen-style core — overlaps 3.7).

### 3.11 Retention / compaction (caps.retention)

- Truncating/compacting old runs drops them but keeps recent ones.
- Indexes/queries stay correct after compaction (no dangling references, ordering
  intact).
- Active (queued/running) runs are never compacted away regardless of age.

### 3.12 Load smoke (optional, all)

- Bulk-insert many runs and measure claim throughput / listing latency. Not a
  correctness gate; a regression signal and a way to surface index gaps per backend.

---

## 4. Beyond example-based tests

- **Property tests (proptest):** the materialization and claim invariants are ideal
  for property testing — generate random interleavings of insert/claim/transition and
  assert "no run claimed twice", "no lost run", "state only moves forward". Ganglion
  already does this style (running-max state-machine model); mirror it for `Store`.
- **Jepsen-style for distributed backends:** for Ganglion-coordinated and distributed
  SQLite, borrow Ganglion's `tests/jepsen` scenario pattern — partition, kill, restart
  while asserting exactly-once materialization and single-claim hold. These are the
  tests that justify the HA claims; reuse the harness rather than reinventing it.
- **Deterministic seeds:** every randomized test logs its seed so a failure is
  reproducible.

---

## 5. Suggested build order

1. Define `Store`-shaped fixtures + `Capabilities` and stand up groups 3.1-3.4
   against the existing `PgStore`. This pins the contract Postgres already satisfies
   and gives the suite teeth immediately.
2. Add 3.5/3.6/3.8/3.9 against Postgres (it supports them all) — now the suite encodes
   the full single-cluster behavior.
3. Bring up the SQLite backend and run the suite; let the capability matrix show
   exactly what single-node SQLite does and does not cover.
4. Add 3.7/3.10/3.11 and the property + Jepsen layers as the Ganglion / distributed
   SQLite backends come online.

The payoff: every new backend is graded against the same behavioral bar, the
capability matrix makes the gaps explicit instead of hidden, and "does Mongo/Ganglion
/distributed-SQLite actually work as an Arbiter backend?" becomes a single
`cargo test` answer.
