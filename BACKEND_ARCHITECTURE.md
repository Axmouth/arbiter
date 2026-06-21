# Arbiter Backend Architecture: Alternative Implementations

Status: design exploration (no code committed from this doc). Goal: map out what
alternative backend implementations could look like behind Arbiter's `Store` trait,
with primary focus on a **self-contained, no-external-service** deployment
coordinated by **Ganglion**, plus the storage options that pair with it and the
non-Ganglion alternatives for comparison.

"Self-contained" here means: no Redis, no external Postgres, no etcd — a single
binary (or a small cluster of identical binaries) with everything embedded.

---

## 1. Current architecture (baseline)

Today every `arbiter-node` process runs two loops over one shared `PgStore`
(`node/src/main.rs:92-99`):

- **Scheduler loop** (`scheduler/src/lib.rs:8`) — gated each tick by
  `store.am_i_leader()` (`scheduler/src/lib.rs:31`). Only the leader materializes:
  it lists enabled cron jobs, computes the next fire times, and calls
  `insert_job_run_if_missing` per timestamp.
- **Worker loop** (`worker/src/lib.rs:12`) — heartbeats, runs the reaper
  (`reclaim_dead_workers_jobs`, currently on every node), then claims due runs
  (`claim_job_runs`) up to local capacity and spawns them.

The abstraction seam is the `Store` trait family in `core/src/lib.rs:427`:
`JobStore`, `RunStore`, `WorkerStore`, `ApiStore`. A backend is an implementation of
these. `PgStore` is the only impl today.

### What Postgres is actually providing

Postgres is one ACID box doing **five distinct jobs**. Any backend has to rehome all
five:

| # | Concern | How PG does it today | Where it lives |
| --- | --- | --- | --- |
| 1 | Leader election | `pg_try_advisory_lock(134037)` | `store-pg` `am_i_leader` |
| 2 | Materialization dedup | `INSERT ... ON CONFLICT(job_id, scheduled_for)` + unique index `job_runs_unique_run` | `insert_job_run_if_missing` |
| 3 | Work queue | `SELECT ... WHERE state='queued' AND scheduled_for<=now() FOR UPDATE SKIP LOCKED` | `claim_job_runs` |
| 4 | Liveness + reaper | `workers.last_seen` heartbeats + requeue stale workers' runs | `heartbeat` / `reclaim_dead_workers_jobs` |
| 5 | Bulk durable + queryable store | tables + indexes; `list_recent_runs` with cursors/filters | `ApiStore` |

The hard one to replace is **#5 with #2/#3 on top**: a durable, *queryable*,
*concurrently-claimable* store. Coordination (#1, #4) is comparatively easy to move.

### 1.1 Data taxonomy: every kind of data and how to handle it

The single most useful design move is to stop treating "the data" as one thing.
Arbiter stores six distinct *kinds* of data with completely different write rates,
durability needs, and query patterns. The ideal backend handles each differently.

| Data kind | Write rate | Cardinality | Query pattern | Consistency tier | Ideal handling |
| --- | --- | --- | --- | --- | --- |
| Job definitions (+ runner cfg, env, shared cfg) | Very low (admin edits) | Low (10s-1000s) | By id; list enabled cron jobs | Strong, must be HA | Replicated record store; cache in memory |
| Run control record (state, worker_id, scheduled_for, epoch) | Medium (per fire + transitions) | Medium-high, but bounded by retention | Claim (due+queued), by id | Strong, single-writer, fenced | Consistent/claimable store; indexed |
| Run payload (output, log, error_output, config_snapshot) | Medium (write-once at finish) | Same rows, large blobs | By run id (detail view) | Best-effort acceptable | Bulk/append store or object storage; off the hot path |
| Worker registry + liveness (last_seen) | Very high (heartbeat every ~2s) | Low (workers) | By id; list live | Soft, ephemeral | Coordination plane (Ganglion labels), NOT the record store |
| Users / auth | Very low | Tiny | By username/id | Strong, must be HA | Replicated record store; unique constraint |
| Cluster control docs (pause, concurrency caps, replication positions, ownership) | Low | Tiny | Read on every loop | Strong, replicated | Ganglion `attributes` / assignments |

#### Per-kind detail

- **Job definitions** are read-heavy, rarely written, small, and *must not be lost or
  diverge* (they decide what runs). Treat them as Tier-1 consistent data: replicate
  them everywhere, and cache them in memory with invalidation on change (the
  scheduler already wants this — see the TODO at `scheduler/src/lib.rs:16`). Small
  enough that even Ganglion `attributes` or a single low-rate replicated path can
  hold them if you want them on the coordination plane.

- **Job runs are two datasets wearing one table.** This is the key realization.
  `job_runs` (`docker/init/000_schema.sql:154`) mixes:
  - a hot **control record** — `state`, `worker_id`, `scheduled_for`, `queue`, and
    (new) the fencing `epoch`. Small, frequently updated, on the critical path,
    must be strongly consistent and single-writer so claims and transitions are
    race-free.
  - a cold **payload** — `output`, `log`, `error_output`, `config_snapshot`. Large,
    written essentially once (at finish), read only in a detail view, and *losing the
    last few is survivable*.

  Ideal handling splits them: keep the control record in the consistent, claimable,
  replicated store; push the payload to a bulk/append store (Keratin segment, object
  storage, or a separate best-effort table) keyed by run id, written off the hot
  path. This is what makes HA tractable — you replicate the small hot thing strongly
  and let the big cold thing be best-effort (section 5.3). `config_snapshot` is a
  subtlety: it must exist *before* execution (the worker aborts without it,
  `worker/src/lib.rs:91`), so it is written at materialization and belongs with the
  control record, not the post-run payload.

- **Run payload** is append-mostly and time-ordered, which is exactly Keratin's
  sweet spot (append + truncate-for-retention). Even on SQLite, consider a separate
  table or `ATTACH`ed db so retention/vacuum of history does not contend with the hot
  control rows.

- **Worker liveness** should never live in the record store. A heartbeat every ~2s
  (`node/src/main.rs:68`) per worker is pure churn with no historical value. It is
  *coordination*, not *data* -> Ganglion node labels with generation-silent refresh.
  Keeping it out of the store also removes the highest-frequency write from whatever
  storage engine you choose, which matters most for the log-structured option.

- **Users/auth** are tiny but Tier-1 (a lost admin user locks you out). Strong,
  replicated, unique-username enforced (in-memory index under single writer if the
  store has no native unique constraint).

- **Cluster control docs** are new with the Ganglion model: global pause, per-job
  concurrency caps, replication positions, ownership/epochs. Tiny, read constantly,
  must be consistent -> Ganglion `attributes` and `PartitionAssignment`. These did
  not exist as first-class data under the PG model (they were implicit); making them
  explicit replicated docs is part of the migration.

#### The tiering that falls out

- **Tier 1 (strong + HA, tiny):** definitions, users, control docs, run *control*
  records. Replicate strongly.
- **Tier 2 (best-effort, bulk):** run *payload* (output/log/snapshot-after-run).
  Local + async export; a dead node loses recent detail, not correctness.

This split is what turns "replicate a whole database" (hard) into "replicate a few
small tables, ship the rest best-effort" (tractable). Every backend below should be
evaluated on how cleanly it lets you express these two tiers.

---

## 2. Decomposition: a backend is a choice on three axes

A "backend" is not one thing. It is a choice along three independent axes:

1. **Coordination plane** — who is leader, who is alive, who owns what, fencing.
2. **Work distribution** — how due runs get to exactly one worker.
3. **Durable store** — where definitions and run history live, and how they are
   queried and (optionally) replicated.

Postgres happens to fill all three at once. Decoupling them is what unlocks the
self-contained story, because the three axes have different best answers.

| Axis | Options considered below |
| --- | --- |
| Coordination | DB-native (today), **Ganglion**, static single-node, external (etcd/consul) |
| Work distribution | shared claimable queue (pull), Ganglion ownership (push), Fibril work queue |
| Durable store | Postgres, **SQLite**, **self-made log-structured (Keratin)**, redb/sled/fjall, Mongo |

---

## 3. Coordination plane: Ganglion (primary focus)

Ganglion is a neutral, embeddable coordination library (Raft via openraft) that is
already validated in production via Fibril. It provides a small replicated
`CoordinationSnapshot` (nodes, resource assignments with owner/followers/epoch,
opaque attributes, generation) plus a watch stream, guarded CAS writes, epoch
fencing, and a controller-loop helper. It is *not* a data store — which is exactly
why it pairs with a separate storage axis.

### 3.1 Mapping Arbiter's coordination needs onto Ganglion

| Arbiter need | Ganglion mechanism | Notes |
| --- | --- | --- |
| Leader election | Raft leader (`is_leader`/`current_leader`/`wait_for_leader`) OR ownership of a `scheduler` resource | Replaces advisory lock. Two-level model below. |
| Liveness | Workers register as Ganglion nodes; heartbeat via generation-silent label refresh; observe via `watch_committed()` | Replaces `workers.last_seen` churn. |
| Reaper trigger | Controller (raft leader) detects stale nodes and reassigns/requeues | Replaces per-worker `reclaim_dead_workers_jobs`. |
| Ownership + fencing | `PartitionAssignment { owner, followers, epoch }`; `next_assignment_epoch` bumps on owner change | Epoch is the fence token. |
| Failover orchestration | `plan_and_propose_guarded(node, plan, retries)` — read, pure-plan, stamp epochs, CAS, retry | Custom planner closure for promotion logic. |
| Reactivity | `watch_committed() -> watch::Receiver<CoordinationSnapshot>` | The central integration surface. |
| Small replicated control docs | `attributes` (set / CAS) | e.g. global pause, concurrency caps, replication positions. |

### 3.2 Two-level leadership (important)

There are two distinct notions of "leader":

- **Ganglion raft leader** — accepts metadata writes, runs the controller loop.
- **Application primary / owner** — the node that owns a given resource
  (`PartitionAssignment.owner`), e.g. "the scheduler" or "shard 7".

These need not be the same node. The raft leader is just who *plans*; the assigned
owner is who *acts*. This decoupling is what makes Ganglion fit ownership problems
cleanly. Arbiter's scheduler-leader becomes "owner of the `scheduler` resource";
shard owners become "owner of `shard:N`".

### 3.3 Architectures on top of Ganglion

**Architecture A — Ganglion coordination + shared claimable queue (minimal change).**

Keep the pull-queue model. Ganglion replaces only coordination:

- `am_i_leader()` -> "am I the raft leader" or "do I own the `scheduler` resource".
- Liveness/reaper -> Ganglion node liveness + a controller reaper loop.
- Materialization, claiming, and storage stay in the `Store` impl (Postgres today,
  or any store that supports an atomic claim).

Pros: smallest delta to `scheduler`/`worker` logic; preserves the pull queue's
*natural load balancing* (an idle worker can grab a hot job). Cons: still needs a
durable, claimable queue + queryable store underneath — Ganglion does not provide
that, so you still pick a storage backend that can do an atomic claim (SQLite via a
transactional `UPDATE ... RETURNING`, or Fibril for the queue).

**Architecture B — Ganglion-native ownership (push).**

Partition jobs into N shards as `ResourceIdentity` (e.g. `arbiter/jobs/shard/N`).
Ganglion assigns each shard to a live worker with an epoch. Each worker
*exclusively* materializes and executes the due runs for its own shards from local
state. No cross-worker claiming, no `SKIP LOCKED`. Failover = Ganglion reassigns a
dead worker's shards with a bumped epoch; the new owner picks up via
`watch_committed()`.

Pros: conceptually cleanest, fewest moving parts (no separate queue), concurrency
limits become local. Cons: loses free load balancing (a hot shard cannot spill to an
idle worker unless shards are fine-grained and rebalanced); failover requires the
shard's run-state to be recoverable by the new owner, which means the store must be
replicated or shared.

**Architecture C — central placement (sticky assignment).**

The leader acts as a placement engine, and the *unit of assignment* is the key knob,
ranging from coarse to fine:

- **Job-level (coarse):** assign whole job definitions; one owner runs all of a job's
  instances. Free serialization for `max_concurrency=1` (single ordered owner), but a
  single hot job cannot spread across nodes.
- **Appointment-level (fine):** assign *(job spec, appointment)* pairs, where an
  appointment is a scheduled occurrence `(job_id, scheduled_for)`. The owning node
  reads the spec and materializes the full run itself. Each occurrence is placed
  independently, so even one job with many concurrent runs spreads across the cluster.
  This is the more flexible form.

In both cases the worker holds the full (Tier-1, replicated) job-definition list and
materializes its own runs; **runs never enter Raft.**

**Critical implementation note (appointment-level):** do NOT replicate per-appointment
assignments through Ganglion's snapshot -- thousands of occurrences/minute = thousands
of snapshot commits, which is exactly the high-churn/cardinality problem that keeps
runs off Raft. An appointment is light as a *payload* but just as churny as a run if
treated as *replicated assignment state*. Realize it one of two ways instead:

- **Deterministic placement over Ganglion membership (recommended).** Every node
  computes the same function from the membership + epoch Ganglion already replicates:
  `owner(job_id, scheduled_for) = weighted_rendezvous(live_nodes, key)`. No
  per-appointment coordination: each node independently materializes and runs the
  occurrences that hash to it. A dead node drops from `live_nodes` and its occurrences
  rehash to survivors -- failover for free; rendezvous/consistent hashing reshuffles
  only ~1/N on a membership change, not all. Load-aware *without* per-appointment
  Raft: publish a small per-node **weight vector** (capacity / live load) as a low-rate
  Ganglion attribute and feed it into the weighted hash; weights change slowly under
  hysteresis while occurrences place locally.
- **Leader-push off-Raft.** The leader computes due occurrences and pushes
  `(job_id, ts)` to workers via a side channel (direct RPC or a Fibril queue), using
  Ganglion only for election + liveness. Fully dynamic per-occurrence load decisions,
  at the cost of a dispatcher bottleneck + the off-Raft path.

The per-job knob unifies coarse and fine: hash the **appointment** to spread a
concurrent job; hash (or pin) the **job** to serialize a `max_concurrency=1` job --
same mechanism, different key, chosen per spec.

Dedup/safety is unchanged either way: for a given membership epoch exactly one node
owns each occurrence, so it materializes once; the handoff window during a membership
change is covered by the epoch fence + the idempotent insert on
`(job_id, scheduled_for)`.

C's defining difference from A is *how failover works*, which is the deciding factor:

- **Pull queue (A) fails over by lease timeout.** A dead worker's lease expires and
  the run is redelivered. The death decision is implicit, local, per-message, and
  uncoordinated -- so it fires even when the worker is alive-but-slow or partitioned
  (spurious double execution), and the lease length is a guess (too short = false
  redelivery, too long = slow recovery).
- **Central placement (C) fails over by coordinated reassignment.** Ganglion liveness
  declares a node dead *once*, and the controller moves its jobs to another node with
  an epoch bump. The decision is explicit and global; the epoch fences the old
  owner's *recorded* state. Fewer spurious duplicates, and recovery tunes on a single
  liveness threshold instead of per-message lease tuning.

Honest caveat: neither is exactly-once for arbitrary *side effects*. Reassignment
does not physically stop a partitioned old owner's running shell command (it may
already have hit the webhook). The epoch fence protects the *bookkeeping* (run state,
claims), not external effects; true side-effect dedup needs idempotency at the target
and is the job's responsibility (`misfire_policy`). What C buys is *fewer* duplicates
(deliberate reassignment vs aggressive lease redelivery) and fenceable state.

Load balancing in C (less rigid than static ownership suggests):

- Carry per-node load in the Ganglion heartbeat label (generation-silent): active
  task count, declared capacity. The planner reads these.
- Optionally weight jobs by observed cost: the leader aggregates completed-run
  durations from run history (Tier-2 data it already has) into avg-time/variance per
  job, and places to equalize *expected load*, not job count.
- Keep it advisory and off-raft, with **hysteresis + stickiness**: prefer the current
  owner (Ganglion's `DeterministicPartitionPlacement` already does this) and rebalance
  only when imbalance crosses a threshold, or placement thrashes.
- Phase it: start with task-count + capacity; add duration-weighting only if needed.
  (This matches Ganglion's documented load-aware direction: load scores in labels,
  advisory, hysteretic.)

Pros: constraint-aware placement (capacity, `queue` lanes, priority); free per-job
serialization for `max_concurrency=1` (single owner runs instances in order, no
distributed lock); coordinated failover with fewer duplicates. Cons: coarser balance
than per-run dispatch; the leader is the planner (failover-mitigated); a long-running
instance blocks its job's next instance (desired for serial jobs, not for others).

**Choosing among A / B / C:**

- **A (pull queue)** when ad-hoc load balancing dominates and lease-based failover is
  acceptable -- smallest change, best accidental balance. Runs are short bursty shell
  executions today (`worker/src/lib.rs:172`), which favors A's balancing.
- **C (central placement)** when you want coordinated failover, constraint-aware
  placement, and the ability to dial granularity per spec -- the Ganglion-native
  "smart scheduler", and the recommended target for a self-contained HA Arbiter. C
  subsumes B: hash the appointment to spread a hot job, hash/pin the job to serialize
  a `max_concurrency=1` job, all via deterministic placement over Ganglion membership
  with no per-appointment Raft.
- **Hybrid** is the endgame: C owns assignment + coarse balance, with a Fibril queue
  absorbing spillover/overflow execution for fine balance -- defer until C's coarser
  balance proves insufficient.

### 3.4 Concrete `Store`/coordination shape

Pull the coordination concern out of `Store` into a dedicated trait so it is
backend-independent:

```rust
// new: coordination is no longer a Store method
#[async_trait]
pub trait Coordinator {
    async fn am_i_leader(&self) -> Result<bool>;          // raft leader or scheduler-owner
    async fn register_self(&self, me: &WorkerRecord) -> Result<()>;   // heartbeat
    async fn live_workers(&self) -> Result<Vec<Uuid>>;    // from committed snapshot
    fn watch(&self) -> watch::Receiver<ClusterView>;      // react to membership/ownership
    // Arch B only:
    async fn my_shards(&self) -> Result<Vec<ShardId>>;
}
```

- `GanglionCoordinator` implements this over `RaftMetadataNode`:
  `am_i_leader` -> `is_leader()`; `register_self` -> `register_node` with a
  `last_seen` label refresh; `live_workers` -> filter the watched snapshot by label
  freshness; the reaper becomes a controller loop using `plan_and_propose_guarded`.
- `am_i_leader()` is **removed from `WorkerStore`** (`core/src/lib.rs:473`) and the
  scheduler tick calls the `Coordinator` instead (`scheduler/src/lib.rs:31`).
- `DbNativeCoordinator` keeps the advisory-lock/`last_seen` behavior for the Postgres
  backend, so coordination and storage stay independently swappable.

### 3.5 What Ganglion exposes today vs. what you build

Verified present (control plane is covered):

- Election (raft leader + assignable owners), epoch issuance/fencing, race-safe
  reassignment via `plan_and_propose_guarded`, node registry with endpoints,
  position-tracking via labels/attributes, and the `watch_committed()` reactive seam.

Gaps you implement on top (all by Ganglion's "issue, don't enforce" design):

1. **Fencing is cooperative, not enforced.** Ganglion hands out the epoch; it cannot
   stop a fenced node from acting, and a partitioned old owner will not *see* its own
   demotion. The fence must be enforced at the data layer: tag writes/claims/
   WAL-frames with the epoch and have the store/replicas reject stale epochs. For
   Arbiter: a run claim or `last_fired` write should be conditional on
   `epoch >= stored_epoch`.
2. **No built-in liveness expiry.** Ganglion stores nodes and refreshes labels but
   does not evict stale ones. The "node stale -> reassign/requeue" loop is yours
   (controller on the raft leader, comparing `last_seen` labels to a threshold).
   Clock-skew and threshold tuning are on you.
3. **Primary-lease freshness.** Pre-0.9 there is no trustworthy "am I still primary
   right now" lease, so do not do lease-based local-only writes; let the data-layer
   epoch fence decide. openraft 0.9 adds `ensure_linearizable()` (confirmed-leader
   read), which Ganglion can expose to partly close this — see Ganglion's
   `OPENRAFT_MIGRATION.md`.
4. **Placement policy.** Built-in `Deterministic`/`LeastLoaded` optimize load, not
   "promote most-caught-up replica". Encode promotion as the custom planner closure
   passed to `plan_and_propose_guarded`.

---

## 4. Durable store options (the second axis)

Ganglion handles coordination; this axis handles the bulk. These compose freely with
section 3.

### 4.1 SQLite (embedded) — recommended default

SQLite is a *library, not a service*, so a single binary with SQLite embedded is
already "self-contained". It hands back the full query power Arbiter relies on
(`list_recent_runs` cursors/filters, joins to runner-config tables) with zero ops.

- **Single node:** trivial. A `SqliteStore` implementing all four `Store` traits;
  no coordination needed (or Ganglion with one voter as the upgrade seam). This
  alone kills the external-Postgres dependency.
- **HA:** SQLite is single-writer and not natively replicated, so HA needs a
  replication strategy (section 5). The clean one is single-primary + WAL shipping,
  coordinated by Ganglion.
- **Claiming (Arch A):** SQLite has no `SKIP LOCKED`, but a single-writer
  `UPDATE job_runs SET state='running', worker_id=? WHERE id IN (SELECT id ... LIMIT
  ?) RETURNING *` inside a transaction is sufficient under one primary.
- **Uniqueness/dedup:** keep the `(job_id, scheduled_for)` unique index — it still
  works and remains cheap crash-insurance even with a single materializer.

Pros: full SQL, mature, embedded, smallest conceptual gap from Postgres. Cons: HA is
external work; single-writer throughput ceiling (fine for a scheduler).

### 4.2 Self-made log-structured store on Keratin

Keratin is an append-only segmented log: **append, scan, truncate** (non-editable).
That constraint dictates a Bitcask/LSM shape, which fits Arbiter well:

- **Mutations are events, not edits.** A run `queued -> running -> succeeded` is
  three appended records; current value = latest record for that key.
- **State + indexes live in memory.** On apply, maintain the materialized value plus
  the secondary indexes Arbiter actually queries: `BTreeMap<scheduled_for, run_id>`,
  `by_job_id`, `by_worker_id`, `by_state`. That *is* the query engine — sufficient
  because Arbiter's query surface is small and known (`ApiStore`).
- **Boot = scan the log, rebuild indexes.** Same pattern Ganglion uses to rebuild
  its snapshot.
- **Compaction = snapshot live set -> new segment -> truncate.** Soft-delete
  (`jobs.deleted_at`) becomes a tombstone dropped at compaction.
- **Uniqueness/CAS** move to the in-memory index under a single writer (the Ganglion
  leader) — race-free by construction, no DB constraint needed.
- **High-churn updates stay off the log.** Worker heartbeats do NOT get an append
  each; that is liveness -> Ganglion. Keep the log for durable record state only.
- **Truncate is a gift for run history:** append-heavy, time-ordered, recent-biased
  data ages out by truncating old segments — natural retention.

Pros: fully in-house, single binary, retention falls out for free, dovetails with
the in-house stack. Cons: you build the query/index layer and compaction; no ad-hoc
queries beyond the indexes you maintain; HA still needs replication (section 5). If
built, this should be its **own reusable crate** (a peer to keratin/fibril/ganglion),
consumed by Arbiter via the `Store` trait — never baked into Arbiter.

### 4.3 redb / sled / fjall (embedded KV)

Middle ground: a mature embedded key-value/LSM engine gives you durable storage +
range scans without writing the log layer yourself. You still build the typed schema
and secondary indexes (like the self-made option), but on a maintained engine with
transactions. Good compromise if SQLite's SQL is more than you need but you do not
want to own a storage engine.

Pros: mature engine, transactions, range scans, single binary. Cons: still build
indexes/schema; less query power than SQLite; HA still external.

### 4.4 Keep an external DB (Postgres / Mongo)

Not single-service, but the multi-backend story Arbiter originally wanted. Postgres
stays the best choice when you *do* have it (it is genuinely good here). Mongo was an
early target: `findAndModify` covers atomic claim, a TTL/`last_seen` field covers
liveness, and a unique index covers dedup — i.e. Mongo can fill the same five jobs as
PG via `DbNativeCoordinator` + `MongoStore`. RabbitMQ was considered for the queue
but is redundant now that Fibril exists and storage is the real need.

### 4.5 Storage decision table

| Option | Query power | HA difficulty | Single-service | In-house | Best when |
| --- | --- | --- | --- | --- | --- |
| SQLite | Full SQL | Medium (WAL ship) | Yes (lib) | No | Default self-contained |
| Self-made (Keratin) | Fixed indexes | Medium-High (you build) | Yes | Yes | Max sovereignty / dogfood |
| redb/sled/fjall | Range scans | Medium-High | Yes (lib) | No | KV is enough, avoid SQL |
| Postgres | Full SQL | Low (mature) | No | No | You already run PG |
| Mongo | Doc queries | Low-Medium | No | No | Doc model preferred |

---

## 5. HA / replication strategies (cross-cutting)

Coordination (Ganglion) decides *who* is primary; this section is *how* the bulk data
survives a node dying.

### 5.1 Single-primary + WAL shipping (recommended for SQLite HA)

A scheduler's write load is light, so you do not need active-active; you need one
primary + warm standbys (the LiteFS/Litestream pattern):

- **Ganglion**: elects the primary (owner of the `db` resource), tracks standby
  liveness + endpoints + replication position (a low-rate per-node label/attribute),
  fences the old primary on failover via epoch bump.
- **SQLite in WAL mode** on every node.
- **Primary ships WAL frames directly to each standby**, standbys discovered from
  Ganglion's node registry. Point-to-point — no broker needed.
- **Failover**: Ganglion promotes the most-caught-up standby (it knows positions),
  bumps the epoch; the old primary is fenced — its stale-epoch frames are rejected by
  standbys, and it must stop serving as primary.

No Fibril, no Plexus, no new DB project. The fence lives in the WAL-ship protocol
(stale-epoch frames rejected), not in the primary's self-belief.

### 5.2 Event-sourced over Fibril (for full active replication)

Writes go to the leader -> appended to a Fibril topic (the replicated mutation log)
-> every node applies to its local store + index -> reads are always local. This is
CQRS over your own broker, and the cleanest dogfood (Ganglion was built for Fibril;
here Fibril becomes Arbiter's replication bus).

**Caveat that gates this today:** replicating a log to N nodes is **fanout** (every
node sees every record). Fibril is currently a **competing-consumers work queue**
only — each message goes to exactly one consumer, which would give each replica
*half* the mutations (corrupt). This needs **Plexus Stream** (Fibril's planned fanout
feature), or a stopgap of one dynamic queue per replica + publish-to-all. Defer until
fanout exists.

### 5.3 Tiered durability (the pragmatic unlock)

You do not have to replicate everything. Tier by cost-of-loss:

- **Must be HA + consistent (tiny):** job definitions, enable/disable, ownership,
  fence epochs. Small enough to live in Ganglion `attributes` or a single low-rate
  replicated path.
- **Best-effort (bulk):** run history, logs, output. Local per node, async export. A
  dead node loses *its* recent history detail, not correctness.

This collapses the hard part: the expensive-to-replicate data (history) is also the
data you can most afford to make best-effort, and the only must-be-consistent data
(definitions) is cheap. Ship this before attempting full replication.

---

## 6. Hard seams / correctness checklist

Independent of backend choice, these are the places a scheduler gets subtly wrong:

- **Exactly-once materialization.** Single materializer (Ganglion leader or shard
  owner) + idempotent insert keyed on `(job_id, scheduled_for)`. Keep the unique key
  even with one writer.
- **In-flight run recovery on failover.** When owner A dies mid-run and B takes over,
  B must learn what was already claimed/fired. Either shared/replicated run-state
  (B reads it) or accept re-run under an at-least-once contract. Decide and document
  the contract (Arbiter's `misfire_policy` is the natural home).
- **Fence enforcement at the data layer.** Stale-epoch claims/writes/WAL-frames must
  be rejected. Test it: partition the primary mid-write, promote a standby, assert
  the old primary's stale-epoch writes are refused.
- **Read-your-writes after a leader write** (replicated stores): a client that just
  wrote via the primary must not read a stale replica. Route reads-after-write to the
  primary, or wait for the replication position to catch up.
- **Liveness threshold vs clock skew.** `dead_after_secs` (currently 30,
  `node/src/main.rs:69`) must exceed worst-case heartbeat jitter + skew, or healthy
  workers get reaped.

---

## 7. Alternatives beyond Ganglion (honest comparison)

Ganglion is not the only coordination answer. For the single-service goal it is the
best fit, but be clear-eyed:

| Coordination | Self-service? | Pros | Cons |
| --- | --- | --- | --- |
| **Ganglion** | Yes (embedded) | Single binary, ownership+fencing+watch, in-house, validated via Fibril | You build fence enforcement + liveness policy; pre-0.9 no lease |
| **DB-native** (advisory lock / SKIP LOCKED / TTL) | Only if DB is embedded | Simplest; zero new infra when a DB is present | Couples coordination to the DB; SQLite is single-node so no real HA from it |
| **etcd / consul** | No (external service) | Battle-tested, leases, watches | Defeats the single-service goal; another thing to run |
| **raft-rs / hand-rolled raft** | Yes | Full control | You re-implement what Ganglion already is, worse |
| **None (static single node)** | Yes | Trivial; fine for many deployments | No HA; one node materializes and executes |

Key judgement: if you do **not** need HA, the simplest self-contained Arbiter is
**SQLite + no coordination** (single node) — Ganglion is overkill there but is the
seam that lets you grow into HA without rewriting. If you **do** need HA without
external services, **Ganglion + SQLite + WAL shipping** is the sweet spot. etcd only
makes sense if you have already abandoned the single-service constraint.

---

## 8. Recommended phased path

1. **Phase 0 — tighten the seam.** Audit for PG-specific assumptions leaking past the
   `Store` traits; extract coordination (`am_i_leader`, reaper) into a `Coordinator`
   trait so storage and coordination are independently swappable.
2. **Phase 1 — single-node self-contained.** Add `SqliteStore` behind `Store`. No
   coordination (or Ganglion with one voter). Kills the external-Postgres dependency.
   Biggest bang for the least risk.
3. **Phase 2 — Ganglion coordination plane.** Add `GanglionCoordinator`: replace the
   advisory lock and the per-worker reaper with raft-leader election + a controller
   reaper. Still a single shared/standalone store. Architecture A.
4. **Phase 3 — HA storage.** SQLite + WAL shipping coordinated by Ganglion (5.1), or
   the self-made replicated store. Add the data-layer epoch fence and the failover
   partition test. Full self-contained HA.
5. **Phase 4 (optional) — graduate the storage/replication layer** into its own
   reusable crate if it proves general (the "embeddable HA SQLite, consensus-fenced
   failover" or "log-structured store on Keratin" project). Arbiter stays a consumer.

Throughout: keep Arbiter a *consumer* of infrastructure (Ganglion, Fibril, the
store), never an owner of generic coordination/replication logic — the same
discipline Fibril holds toward Ganglion.

---

## 9. What this thread concluded (deployment reality, durability, strategy)

Three conclusions worth not losing.

### 9.1 Deployment tiers map cleanly to backends

- **Tier 1 -- solo / single box** (scheduler + workers on one machine, often
  colocated with other services). The overwhelming majority of real deployments.
  Served by **single-node SQLite**: one binary, no external DB. Multiple worker
  processes can still share the file.
- **Tier 2 -- workers scaled across machines, shared DB.** The "outgrew one box"
  case. Served by **Postgres** (or Mongo) -- arbiter's current model.
- **Tier 3 -- HA control plane, no external deps.** Genuinely rare. Served by
  **Ganglion-coordinated** backends.

Tiers 1+2 (SQLite + Postgres) cover almost everyone. Tier 3 is demand-pulled, not
enthusiasm-pushed; the architectures above are its blueprint for when the pull comes.

### 9.2 Durability != availability (and durability wins)

The solo/colocated majority accepts single-points-of-*availability* (box dies =
everything down, acceptable) but expects *durability* (restart/crash = data intact).
So effort priority is **durability + crash recovery >> distribution**: SQLite in WAL
mode, correct fsync, in-flight runs recovered on restart (conformance group 3.9). HA
is a separate, rarer want -- do not let "what about node failure?" steal effort from
"does it survive a restart?", which is the question users actually have.

### 9.3 Work distribution is a per-backend strategy, not a Store method

`claim_job_runs` bakes in a Postgres assumption: a shared multi-writer store with
cheap concurrent row locks. Backends without those properties (single-primary
distributed SQLite) reveal it as the wrong altitude. Work distribution is a
*strategy* the backend brings:

- **DB-native (PG, Mongo):** claim/pull (SKIP LOCKED / conditional update). Their
  native strength.
- **Ganglion-coordinated:** deterministic placement (section 3.3 C). Single-writer
  storage *helps* placement and *hurts* pull.

So the polymorphism should not stop at storage: a `Dispatcher`/`Coordinator` strategy
sits alongside the `Store` impl, and `claim_job_runs` becomes one strategy's detail,
not a universal contract. Note single-node SQLite still uses the claim strategy -- it
is multi-worker (section on the test suite); only *distributed* SQLite would switch to
placement.

---

## Appendix: concern -> mechanism quick map

| Concern | Postgres (today) | Ganglion + SQLite (target) |
| --- | --- | --- |
| Leader election | advisory lock | raft leader / `scheduler` owner |
| Materialization dedup | unique index + ON CONFLICT | single materializer + unique index |
| Work queue | SKIP LOCKED claim | single-primary `UPDATE ... RETURNING` (Arch A) or shard ownership (Arch B) |
| Liveness | `last_seen` column | Ganglion node labels (silent refresh) |
| Reaper | per-worker `reclaim` | controller loop on raft leader |
| Bulk store + queries | tables + indexes | SQLite tables + indexes |
| HA | PG replication (external) | WAL shipping fenced by epoch |
