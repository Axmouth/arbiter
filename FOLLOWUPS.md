# Arbiter Follow-ups

A single tracked home for deferred work, so it is not scattered across code TODOs.
Order roughly reflects intent: lock in a reliable scheduler first, then wire up the
partial features, then differentiate.

## 1. Finish the reliable core (lock-in + wire the partials)

These are mostly "modeled but not enforced" — the credibility gap between demo and
production.

- **Enforce `max_concurrency`.** `worker::count_local_running_tasks()` is a stub
  returning `0`, so per-job concurrency is not actually limited.
- **Misfire policies.** Defined (`MisfirePolicy`) but not enforced; also startup
  catch-up ("jobs with no runs in the last N windows", `scheduler/src/lib.rs:12`).
- **Retries + timeouts.** Per-job retry policy; job execution timeout
  (`worker/src/lib.rs:187`).
- **Reaper placement.** Run the reaper only on the leader/reaper node, not every
  worker (`worker/src/lib.rs:40`).

## 2. Prearming (priority feature)

Prepare slow per-run ceremony (SSH handshake, DB connect, HTTP auth token, docker
pull) *ahead* of the scheduled instant, then fire the action precisely on time with
no setup latency in the way. (Seeded in `worker/src/lib.rs:170-171`.)

Design shape (build on the proven core, not before it):

- **Claim window:** claim runs where `scheduled_for - prearm_lead <= now`, not just
  `scheduled_for <= now`. Worker prepares, then fires at `scheduled_for` via a local
  timer.
- **Run lifecycle gains a phase:** `queued -> armed/preparing -> running -> terminal`.
  An armed run is owned but not yet fired (do not overload `running`).
- **Reaper extends:** an armed run whose worker dies mid-prep must be recoverable
  with the same fence logic. Needs conformance coverage for the armed state.
- **Pairs with shared runner configs (below):** prearm should become "ensure the
  pooled connection is ready / check it out" against a warm per-config connection
  pool, amortizing setup across many runs — not a fresh open each time.
- **Failure handling:** prearm gives a window to detect setup failure before the fire
  instant and apply the misfire policy (skip / alert) instead of firing into failure.
- **Differentiator:** Cronicle runs the plugin and lets it set up inline; warm pools
  + precise firing is something it does not do.

## 3. Runners + shared configs

- **Runners beyond shell:** HTTP, Postgres/MySQL, Python, Node (modeled in the schema
  and `RunnerConfig`, executed only for shell today —
  `worker/src/lib.rs:106`, `store-sqlite` returns "not supported" for non-shell).
- **Shared runner configs:** DB credentials, HTTP auth, SSH config shared between jobs
  (`worker/src/lib.rs:168`, schema). Natural home for the warm connection pools that
  prearming uses.
- **Secrets:** a `Secret` type that holds an id and resolves from storage at the last
  moment (`core/src/lib.rs:10`); secret-key rotation (`cli/src/main.rs:5`).

## 4. To surpass Cronicle (high-value UX/features)

Cronicle's foundation (Node runtime, bespoke flat-file storage) is weaker than
arbiter's (single binary, real DB); the gap is features/UX. Close the high-value 20%,
skip the long tail.

- **Real-time log capture + streaming** — the biggest day-to-day UX gap. Live tail of
  a running job's output. (Log storage is a TODO; streaming via SSE/WebSocket.)
- **Failure notifications** — webhook first, email later.
- **Job/event chaining** — run B on success/failure of A (defer if forced; high value).
- The runner breadth + retries/timeouts/concurrency above are also part of parity.

## 5. Workflows (future)

Workflow-style features (chaining, fan-out/fan-in, conditional steps, DAGs) are noted
for later, after the reliable scheduler and prearming. Keep them out of the core until
the single-job lifecycle is rock solid.

## 6. Storage / backends

- **Retention / pruning:** hard-prune done -- `RunStore::prune_runs(older_than)` deletes
  terminal runs (succeeded/failed/cancelled) older than a cutoff, sparing active runs,
  on both backends, with a conformance `retention` group. Remaining: wire it into the
  worker maintenance loop (leader-gated + throttled + configurable window,
  `worker/src/lib.rs:46-47`); and a soft-delete window for runs (needs a
  `job_runs.deleted_at` column) for the "smaller index, keep history longer" case.
- **store-pg `am_i_leader` fragility:** uses `pg_try_advisory_lock` on a *pooled*
  connection; advisory locks are session-scoped, so repeated calls on one node can
  route to different connections and flip true/false. Cross-node exclusivity is fine
  (and tested); single-node stability is not. SQLite's lease row is unaffected. A
  future "leader stable across repeated calls" conformance case would expose it.
- **`output` representation mismatch:** stored as JSONB in Postgres but `Option<String>`
  in the model / TEXT in SQLite. Not currently asserted by any case; unify the contract.
- **Non-shell runners in store-sqlite:** `create_job`/`update_job` return "not supported"
  for non-shell; implement once those runners are wired.
- **Conformance additions:** retention group (when the method lands); leadership
  stability case (would surface the `am_i_leader` issue); fencing/HA groups are future
  (`multi_node` — Ganglion / distributed SQLite).

## 7. Model / types

- Cron as a real type, not `String` (`core/src/lib.rs:69`).
- `JobRun` possibly as a state enum for tighter state alignment (`core/src/lib.rs:349`).
- Stricter HTTP runner types (`core/src/lib.rs:206`).

## 8. API / admin / cluster

- Node-management endpoints + a cluster-join protocol (`api/src/main.rs:67`).
- Per-node config via the admin UI; per-node dashboard (`node/src/main.rs:27`).
- The cluster of TODOs in `api/src/routes.rs` (auth/endpoints).

## 9. UI (no changes yet; queued)

- Live log streaming view + run detail.
- Job duration graphs; per-worker run listing / worker detail.
- Import/export of job definitions.
- Surface prearm/armed status and misfire/retry config once those land.

## 10. CI / Docker follow-ups

- Add a `/health` endpoint and a Dockerfile `HEALTHCHECK` (omitted until the route
  exists).
- A tag/release workflow (publish versioned images/binaries on git tags).
- Build and embed the web-ui SPA into the `arbiter-api` image.

## Schema notes (`docker/init/000_schema.sql`)

Scheduled-vs-template jobs, retry/acceptance config, name uniqueness, HTTP-runner auth
step, SSH prepare step, misfire-policy storage rework + constraints.

## Resolved (kept for the record)

- **store-pg `update_user` reversed `COALESCE`** — `COALESCE(col, $n)` kept the old
  value, silently dropping updates. Fixed to `COALESCE($n, col)`. Caught by the new
  `crud::update_user` conformance case.
