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

- `[PLANNED]` Enforce `max_concurrency`. `worker::count_local_running_tasks()` is a stub
  returning `0`, so per-job concurrency is not actually limited.
- `[PLANNED]` Misfire policies. Defined (`MisfirePolicy`) but not enforced; also startup
  catch-up ("jobs with no runs in the last N windows", `scheduler/src/lib.rs:12`).
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

## Resolved (kept for the record)

- `[DONE]` store-pg `update_user` reversed `COALESCE` — `COALESCE(col, $n)` kept the old
  value, silently dropping updates. Fixed to `COALESCE($n, col)`. Caught by the
  `crud::update_user` conformance case.
