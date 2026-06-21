# Arbiter Follow-ups

A single tracked home for deferred work, so it is not scattered across code TODOs.
Order roughly reflects intent: lock in a reliable scheduler first, then wire up the
partial features, then differentiate.

## Status legend

Every item below starts with exactly one bracketed stage tag, so the file stays
greppable / mechanically accessible (e.g. `grep '\[PLANNED\]' FOLLOWUPS.md`).

- `[DONE]` — complete and verified.
- `[WIP]` — actively in progress.
- `[PLANNED]` — committed, not started.
- `[IDEA]` — under consideration / future direction, not yet committed.
- `[BLOCKED]` — cannot proceed until a dependency is resolved.

These are the only valid stages. If none of them sufficiently describes an item's
state, add a new stage to this list before using it — never invent an ad-hoc status
inline. Keeping the vocabulary closed is what makes the file greppable.

## 1. Finish the reliable core (lock-in + wire the partials)

Mostly "modeled but not enforced" — the credibility gap between demo and production.

- `[DONE]` Worker capacity. The worker tracks in-flight run tasks (an RAII counter) and
  claims only up to `cfg.capacity`, instead of over-spawning (the old
  `count_local_running_tasks()` stub returned 0).
- `[PLANNED]` Per-job `max_concurrency` (the `JobSpec` field) is still not enforced at
  claim. Enforcing it changes the claim contract -- the conformance suite deliberately
  claims many runs of a `max_concurrency=1` job -- so it needs a claim rewrite on both
  backends (PG cannot combine `FOR UPDATE` with window functions, so lock-then-filter)
  plus best-effort handling of cross-worker races. Do it as its own pass: raise the
  `seed_job` concurrency in existing claim cases, then add dedicated max-concurrency
  cases.
- `[DONE]` Misfire policies. The scheduler scans a bounded look-back window
  (`[scheduler] misfire_catchup_secs`, `0` = disabled) and applies each job's
  `MisfirePolicy` to missed fires: Skip / RunAll / Coalesce / RunImmediately (collapse
  to the latest missed) / RunIfLateWithin(d). Decision logic is pure and unit-tested in
  the scheduler crate; backfill goes through the idempotent insert. `misfire_catchup_secs`
  caps the unbounded family (RunAll / Coalesce / RunImmediately); self-windowed policies
  (RunIfLateWithin(d)) use their own window regardless of the cap, so a per-job policy
  works without an operator enabling global catch-up.
- `[IDEA]` Richer misfire variants: windowed/count-based policies, e.g. "run all missed
  in the last N min" (`RunAllWithin(d)`), "run only if fewer than K were missed",
  or decisions relative to the next upcoming fire (skip a stale run if the next one is
  imminent). Each is a new arm on the per-job `MisfirePolicy` enum + `select_misfire_fires`
  match (+ a unit test); self-windowed, so still bounded by the cap. No loop/storage
  changes needed.
- `[PLANNED]` Retries + timeouts. Per-job retry policy; job execution timeout
  (`worker/src/lib.rs:187`).
- `[PLANNED]` Reaper placement. Run the reaper only on the leader/reaper node, not every
  worker (`worker/src/lib.rs:40`). (Retention pruning is already leader-gated; fold the
  reaper into the same pattern.)

## 2. Prearming (priority feature)

- `[PLANNED]` Prepare slow per-run ceremony (SSH handshake, DB connect, HTTP auth token,
  docker pull) *ahead* of the scheduled instant, then fire precisely on time with no
  setup latency. (Seeded in `worker/src/lib.rs:170-171`.)

  Design shape (build on the proven core, not before it):
  - Claim window: claim where `scheduled_for - prearm_lead <= now`; prepare, then fire at
    `scheduled_for` via a local timer.
  - Run lifecycle gains a phase: `queued -> armed/preparing -> running -> terminal`
    (do not overload `running`).
  - Reaper extends: an armed run whose worker dies mid-prep must be recoverable with the
    same fence logic. Needs conformance coverage for the armed state.
  - Pairs with shared runner configs (below): prearm becomes "ensure the pooled
    connection is ready" against a warm per-config pool, not a fresh open each time.
  - Failure handling: prearm gives a window to detect setup failure before the fire
    instant and apply the misfire policy instead of firing into failure.
  - Differentiator vs Cronicle, which sets up inline.

## 3. Runners + shared configs

- `[PLANNED]` Runners beyond shell: HTTP, Postgres/MySQL, Python, Node (modeled in the
  schema and `RunnerConfig`, executed only for shell today — `worker/src/lib.rs:106`,
  `store-sqlite` returns "not supported" for non-shell).
- `[PLANNED]` Shared runner configs: DB credentials, HTTP auth, SSH config shared between
  jobs (`worker/src/lib.rs:168`, schema). Natural home for the warm connection pools that
  prearming uses.
- `[PLANNED]` Secrets: a `Secret` type that holds an id and resolves from storage at the
  last moment (`core/src/lib.rs:10`); secret-key rotation (`cli/src/main.rs:5`).

## 4. To surpass Cronicle (high-value UX/features)

Cronicle's foundation (Node runtime, bespoke flat-file storage) is weaker than arbiter's
(single binary, real DB); the gap is features/UX. Close the high-value 20%.

- `[PLANNED]` Real-time log capture + streaming — the biggest day-to-day UX gap. Live
  tail of a running job's output (storage TODO; streaming via SSE/WebSocket).
- `[PLANNED]` Failure notifications — webhook first, email later.
- `[IDEA]` Job/event chaining — run B on success/failure of A (high value; defer if
  forced).

## 5. Workflows

- `[IDEA]` Workflow-style features (chaining, fan-out/fan-in, conditional steps, DAGs),
  after the reliable scheduler and prearming. Keep out of the core until the single-job
  lifecycle is rock solid.

## 6. Storage / backends

- `[DONE]` Retention / pruning. `RunStore::prune_runs(older_than)` on both backends
  (conformance `retention` group); leader prunes on an interval in the worker loop,
  configured via `[retention]` (`run_retention_days`, `prune_interval_secs`; `0` days
  disables); `POST /api/v1/runs/prune?older_than_days=N` triggers a manual prune.
  - `[PLANNED]` Soft-delete window for runs (needs a `job_runs.deleted_at` column) for the
    "smaller index, keep history longer" case.
  - `[PLANNED]` UI for retention — see UI section.
- `[DONE]` store-pg `am_i_leader` now uses a lease row (a `leader_lease` table, matching
  SQLite) instead of a pooled, session-scoped advisory lock, fixing single-node
  stability and giving TTL-based failover. Covered by `leadership::stable_across_calls`
  on both backends.
- `[PLANNED]` `output` representation mismatch: stored as JSONB in Postgres but
  `Option<String>` in the model / TEXT in SQLite. Not currently asserted by any case;
  unify the contract.
- `[PLANNED]` Non-shell runners in store-sqlite: `create_job`/`update_job` return "not
  supported" for non-shell; implement once those runners are wired.
- `[PLANNED]` Conformance additions: fencing/HA groups are future (`multi_node` —
  Ganglion / distributed SQLite).

## 7. Model / types

- `[PLANNED]` Cron as a real type, not `String` (`core/src/lib.rs:69`).
- `[IDEA]` `JobRun` possibly as a state enum for tighter state alignment
  (`core/src/lib.rs:349`).
- `[PLANNED]` Stricter HTTP runner types (`core/src/lib.rs:206`).

## 8. API / admin / cluster

- `[PLANNED]` Node-management endpoints + a cluster-join protocol (`api/src/main.rs`).
- `[PLANNED]` Per-node config via the admin UI; per-node dashboard (`node/src/main.rs`).
- `[PLANNED]` The cluster of TODOs in `api/src/routes.rs` (auth/endpoints).
- `[PLANNED]` Role-gate destructive admin endpoints (e.g. `runs/prune`) once role checks
  exist; today they only require auth.

## 9. UI (no changes yet; queued)

- `[PLANNED]` Live log streaming view + run detail.
- `[PLANNED]` Job duration graphs; per-worker run listing / worker detail.
- `[PLANNED]` Import/export of job definitions.
- `[PLANNED]` Surface prearm/armed status and misfire/retry config once those land.
- `[PLANNED]` Retention: a settings control for the window (`run_retention_days` /
  `prune_interval_secs`) and a manual "prune now" action. Backend is ready:
  `POST /api/v1/runs/prune?older_than_days=N` returns the count pruned.

## 10. CI / Docker follow-ups

- `[PLANNED]` Add a `/health` endpoint and a Dockerfile `HEALTHCHECK` (omitted until the
  route exists). (Note: an API `/health` route already exists; wire the Docker
  healthcheck to it.)
- `[PLANNED]` A tag/release workflow (publish versioned images/binaries on git tags).
- `[PLANNED]` Build and embed the web-ui SPA into the `arbiter-api` image.

## 11. Schema (`docker/init/000_schema.sql`)

- `[PLANNED]` Scheduled-vs-template jobs, retry/acceptance config, name uniqueness,
  HTTP-runner auth step, SSH prepare step, misfire-policy storage rework + constraints.

## 12. Runtime (admin-settable) settings

- `[PLANNED]` Move operational knobs from static config into DB-backed, admin-settable
  runtime settings, read **live** at use-time (not captured at startup), to the extent
  feasible. Candidates: `misfire_catchup_secs`, retention (`run_retention_days`,
  `prune_interval_secs`), worker `capacity`, tick/heartbeat intervals, `dead_after_secs`.
  Shape: a settings table (typed key/value) + `Store` get/set, the scheduler/worker
  reading it live each loop, an admin API + UI to view/edit, and conformance coverage.
  Static config stays the bootstrap/default source; runtime settings override at run
  time. The shared DB is the cluster-replication substrate for these settings.

## Resolved (kept for the record)

- `[DONE]` store-pg `update_user` reversed `COALESCE` — `COALESCE(col, $n)` kept the old
  value, silently dropping updates. Fixed to `COALESCE($n, col)`. Caught by the
  `crud::update_user` conformance case.
