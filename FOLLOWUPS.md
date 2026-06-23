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
- `[IDEA]` Calibrate worker capacity and per-job "work weights" over time (a job's cost is
  not 1 slot uniformly). Ties into load-aware routing. Needs the benchmark harness (§6) to
  measure against.
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
- `[DONE]` Retries. Per-job retry policy (`max_attempts` + fixed/exponential/fibonacci
  backoff with mandatory jitter); `retryable` outcomes requeue with backoff (§3a).
- `[DONE]` Timeouts for http/python/node/pgsql/mysql (`timeout_sec`). `[PLANNED]` shell
  runner has no timeout yet (build it on the same `run_with_timeout` shape).
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

- `[WIP]` Runners beyond shell. HTTP runner works end to end on **both** backends
  (worker `execute_http_request`: 2xx = success, other = failure w/ code, transport
  error = failure; SQLite stores http config and builds Http snapshots).
  - `[DONE]` Python/Node execution. Worker runs them via a shared `run_subprocess`
    helper (env injection + per-run timeout + clean stdout/stderr capture, reused by
    the shell runner). Convention: Python `python3 -c "from {module} import {class};
    {class}()"`, Node `node -e "require('{module}').{fn}()"`. SQLite has
    `job_runner_python`/`job_runner_node` + `job_env_vars` tables, create_job/snapshot/
    listing parity, and loads env into shell/python/node snapshots (parity with PG).
    Enforced by conformance `claim::carries_python_snapshot`/`carries_node_snapshot`
    (both backends) and real-execution `worker/tests/full_flow.rs::{python,node}_runner_full_flow`.
  - `[PLANNED]` Richer runner contract (beyond process-level). A defined task
    shape/SDK (implement a class/function returning a structured result) so output,
    error, and logs are first-class instead of stdout-scraping. See §3a.
  - `[DONE]` Postgres/MySQL execution. Worker `execute_pgsql_query`/`execute_mysql_query`
    connect with the resolved password and run the query (success reports rows affected as
    JSON; query error = failed, connection/timeout = retryable). The password is a secret
    reference resolved at execution via `SecretResolver` (the snapshot carries a reference,
    never plaintext). pgsql live-tested against the test PG (success/failed/retryable);
    mysql implemented on the same path. SELECT result-set capture is a future enhancement.
  - `[DONE]` Secret references in env vars: a subprocess runner env value of the form
    `secret:<name>` is resolved at execution (`SecretResolver` wired through worker_tick ->
    spawn_run_task; `node` builds the `SecretManager`). Full-flow tested on SQLite.
  - `[IDEA]` Make subprocess runs (shell/python/node) a bit stateful: persist the child
    PID (+ owning worker/node id) on the run, so a restarted worker can see/clean up what
    was in flight. Caveat: a worker crash usually takes its children down (or orphans them
    to init), so this is mainly for visibility/cleanup, not reliable reattach.
- `[PLANNED]` Shared runner configs: DB credentials, HTTP auth, SSH config shared between
  jobs (`worker/src/lib.rs:168`, schema). Natural home for the warm connection pools that
  prearming uses.
  - `[PLANNED]` Config validation: a "test" action verifying a config works without running
    the job (connect to the DB / open the SSH session / auth the HTTP step), no mutations.
    Surfaced in the config UI (§9).
  - `[PLANNED]` SSH runner + config (run a command over SSH, or SSH-then-run) + UI.
  - `[PLANNED]` HTTP auth workflow: an optional pre-step that fetches a token (request +
    extraction) and applies it (header/cookie) to the run.
- `[IDEA]` Runner output-type validation: for DB runners, prepare/parse the query
  (sqlx/sqlparser) to know the result shape ahead of time. Harder for other runners (could
  learn/remember between runs). Pairs with the richer result contract (§3a).
- `[DONE]` Secrets storage/retrieval (single-node), see §13 / `SECRETS.md`. References use a
  `secret:<name>` string convention (no uuid newtype needed). The store enforces name
  uniqueness (`secrets.name UNIQUE`). When tenancy lands this becomes unique-per-tenant to
  avoid cross-tenant overlap (§14). Rotation is designed (SECRETS step 6).

## 3a. Richer runner contract (structured results, beyond process level)

Today a run yields only `exit_code` + raw stdout/stderr. That cannot separate a
structured **return value** from incidental logging, a **typed error** (class/message/
stack) from a generic non-zero exit, **retryable** vs **permanent** failure, **logs**
(a leveled stream) from **output**, or **progress/heartbeat** for long tasks. This is
Cronicle's weak spot (stdout-scraping). Design as layers, opt-in, keeping process level
as the universal floor:

Chosen design (see `RUNNER_RESULT_PROTOCOL.md`): an injected **language-side runtime**
(Layer B, written in python/node) that imports the user's callable, runs it, marshals the
return value by type, handles errors/logs, and **owns the transport** -- so file -> socket
-> ws is a Layer B/A change, user code (`run(ctx) -> X`) untouched. Vendored now: a single
stdlib-only file the worker writes to temp and invokes (zero pip/npm install).

- `[DONE]` (a) Process level. Exit code + captured streams; runs any script/binary
  (`run_subprocess`).
- `[DONE]` P1 (b) Structured result protocol via injected runtimes. `worker/runtimes/
  arbiter_runtime.{py,js}` (stdlib-only, `include_str!`'d). Worker `ensure_runtime_file`
  writes the runtime **once** to a content-addressed reused path (atomic temp+rename);
  `execute_runtime` hands the child the handshake on **argv** (`--module/--entry/
  --result-file/--run-id/--transport/--protocol`) -- env stays the job's own vars only, so
  no env pollution or grandchild leak. `file` transport: result is a `tempfile` (RAII
  `TempPath` cleanup, owned upstairs). Result doc `{protocolVersion, status, output,
  error{type,message,stack}}`; mapped onto existing `output`/`error_output`/`exit_code`
  columns (no migration yet); falls back to the raw process outcome if absent.
  `prepare`/`run` lifecycle defined (`prepare` inline in v1). Full-flow: python/node
  return-value, structured python dict output, structured node error
  (`worker/tests/full_flow.rs`).
  - `[PLANNED]` Worker-side sweep of crash-orphaned `arbiter-result-*` temp files (TempPath
    covers the normal path; a kill mid-run can orphan one). Fits the maintenance loop.
- `[DONE]` Structured outcome + retry (the data-model half of P2). `job_runs` now has
  `stdout`/`stderr` (text streams) + typed `result`/`result_media_type` and
  `error`/`error_media_type` + `result_status` (success|failed|retryable) + `attempt`;
  output is text + media type (json is just `application/json`), so shell=stdout,
  http=body+Content-Type, runtime=return value (json/text) -- streams stay separate from
  the answer. Per-job retry: `max_attempts`/`backoff_strategy` (fixed|exponential|
  fibonacci)/`backoff_base_secs`/`backoff_cap_secs`, mandatory full jitter
  (`core::next_retry_delay`, unit-tested). Worker maps each runner to a `RunOutcome` and,
  on `retryable` while attempts remain, calls `reschedule_for_retry` (requeue with backoff);
  `finalize_run` records terminal outcomes. HTTP 408/425/429/5xx + transport errors +
  shell `exit 75` (EX_TEMPFAIL) are retryable. Conformance: `outcome::*`, `retry::*` (both
  backends); full-flow: `shell_runner_retries_on_tempfail`, structured python/node
  output/error. API: optional `retry` on create/update job.
- `[PLANNED]` P2 remainder: `socket` transport + resident mode -> prearm; `ARBITER_EVENTS_FILE`
  NDJSON (logs/progress/heartbeat) + `last_heartbeat`/`progress` columns + reaper heartbeat.
- `[PLANNED]` P3: published pip/npm SDK packages; richer `ctx` (params, secrets, artifacts).

Notes: backend-agnostic (worker-side; result lands in run columns). Unifies several planned
items -- JSONB `output` (§6), log streaming (§4/§12), retry policy, progress -> reaper
heartbeat. Raw `-c`/`-e` invocation dropped in favor of the runtime; a no-runtime raw path
can return as an opt-out fallback if needed.

## 4. To surpass Cronicle (high-value UX/features)

Cronicle's foundation (Node runtime, bespoke flat-file storage) is weaker than arbiter's
(single binary, real DB); the gap is features/UX. Close the high-value 20%.

- `[PLANNED]` Real-time log capture + streaming — the biggest day-to-day UX gap. Live
  tail of a running job's output (storage TODO; streaming via SSE/WebSocket).
- `[PLANNED]` Failure notifications — webhook first, email later.
- `[IDEA]` Job/event chaining — run B on success/failure of A (high value; defer if
  forced).
- `[IDEA]` Custom inbound webhooks that trigger (templated) jobs. Needs a mechanism to
  decide which node hosts the endpoint in a cluster.

## 5. Workflows

- `[IDEA]` Workflow-style features (chaining, fan-out/fan-in, conditional steps, DAGs),
  after the reliable scheduler and prearming. Keep out of the core until the single-job
  lifecycle is rock solid.
- `[IDEA]` Job templates with fillable fields, and chaining templated jobs so one job's
  output feeds the next's inputs (the data-passing layer under workflows).

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
- `[DONE]` `output` representation mismatch resolved: unified to **TEXT** on both backends
  (PG `output` JSONB -> the new `stdout`/`result` TEXT columns; SQLite already TEXT). Output
  is text + a media type (`result_media_type`), so json is `application/json` text rather
  than a JSONB column. See §3a. (If native JSON querying is ever needed, add a typed/
  generated column then.)
- `[DONE]` SQLite execution parity. SQLite `claim` now builds + persists + attaches the
  config snapshot (Shell and Http), so the worker can execute SQLite-backed runs; added a
  `job_runner_http` table + `create_job`/listing support. Enforcing conformance cases
  (`claim::carries_shell_snapshot` / `carries_http_snapshot`) fail any backend whose
  claim lacks a usable snapshot -- closing the blind spot that let SQLite pass while
  unable to execute. Full-flow per-runner tests (`worker/tests/full_flow.rs`) run a real
  shell command, a mocked HTTP request, and real python + node runs end to end on SQLite.
- `[DONE]` SQLite env vars: added the `job_env_vars` table; SQLite loads env into
  shell/python/node snapshots (parity with PG's `load_env_for_job`). Subprocess runners
  inject it (`run_subprocess`).
- `[DONE]` Job env-vars Store API: `set_job_env` (replace-all) / `get_job_env` on `ApiStore`
  (both backends); API `GET`/`PUT /api/v1/jobs/{id}/env` + optional `env` on create/update
  (env is a sub-resource, kept out of `JobSpec`). Conformance: `env::set_get_roundtrip`,
  `env::set_replaces_all`, `claim::carries_env_snapshot` (both backends). Future: env values
  may be secret references (§13).
- `[PLANNED]` Cross-backend migration -- **omni-directional** via a canonical dump. Define
  one backend-agnostic serialization (a "grand JSON" with all data: jobs/runs/users/
  settings/env/...). Each backend implements **export -> JSON** and **import <- JSON**, so
  any direction works (SQLite <-> Postgres <-> Ganglion) with N+N impls, not N^2 pairwise
  migrations. Not urgent; first cut a CLI export/import. Define what transfers (in-flight
  runs, id stability, snapshots), and online vs offline/downtime semantics. The uniform
  `Store` trait + conformance suite make this tractable.
- `[PLANNED]` Conformance additions: fencing/HA groups are future (`multi_node` —
  Ganglion / distributed SQLite).
- `[PLANNED]` Scheduler query optimization: materialization is N queries. Aim for a constant
  few (batch the due-jobs scan + batch-insert runs) plus caching/invalidation to cut polling
  and avoid double insertions.
- `[PLANNED]` `list_jobs` N+1: PG loops `load_jobspec_full` per job. Collapse to one query
  (or batched joins).
- `[PLANNED]` Split `store-pg/src/lib.rs` into modules (jobs/runs/workers/secrets/...). It is
  one large file.
- `[IDEA]` Additional backend: MongoDB (the conformance suite makes a new backend tractable).
- `[IDEA]` Benchmark harness: runs/sec materialized + claimed + completed, to guide the
  capacity/weight algos (§1).

## 7. Model / types

- `[PLANNED]` Cron as a real type, not `String` (`core/src/lib.rs:69`).
- `[IDEA]` `JobRun` possibly as a state enum for tighter state alignment
  (`core/src/lib.rs:349`).
- `[PLANNED]` Stricter HTTP runner types (`core/src/lib.rs:206`).
- `[IDEA]` A `Lost` run state for runs whose worker vanished mid-flight (distinct from a
  clean Failed). The schema state check already anticipates adding it.

## 8. API / admin / cluster

- `[DONE]` One binary, role-composed. `arbiter-node` is the single process; it is a
  cluster member with its own identity that runs any subset of `api`/`scheduler`/`worker`
  roles, toggled by `[roles]` config (env `ARBITER_ROLES_*`), default all-on. The `api`
  crate is now a library mounted by the api role (`arbiter_api::run_http_api`), not a
  separate binary. This is what lets the api role hold a KEK (it runs in a node), so
  secret/config/tenant write endpoints can encrypt. Compose has api-only + worker-only
  example services.
- `[PLANNED]` Decouple node identity from worker-table registration: today every node
  (even api-only) goes through `load_or_register_identity`, which inserts a `workers` row
  and locks `/data/worker-id`. An api-only node should get a stable node id without
  registering as a worker. Do this when the worker-only/api-only split needs to be clean
  (multi-node), so api-only nodes do not show up as phantom workers.
- `[DONE]` Multi-node KEK distribution: `SecretManager::reconcile_shares` seals the active
  KEK to every approved node missing a share (`node_keys` -> `kek_shares`), run on a 30s
  task by any node holding the KEK (idempotent). The join side: `load_or_join_kek` in the
  node registers this node's key (via `load_or_bootstrap`) and waits for a holder to seal a
  share, then loads. So a fresh worker or api-only node on another host gets the KEK and can
  resolve/create secrets. Tested end to end in `arbiter-secrets` (A seals to B, B loads).
  `[PLANNED]` admin approval gate: nodes self-register as `approved` today (cluster = trusted);
  a pending->approved flow would gate which keys receive the KEK.
- `[PLANNED]` Node-management endpoints + a cluster-join protocol.
- `[PLANNED]` Per-node config via the admin UI; per-node dashboard (`node/src/main.rs`).
- `[PLANNED]` The cluster of TODOs in `api/src/routes.rs` (auth/endpoints).
- `[PLANNED]` Role-gate destructive admin endpoints (e.g. `runs/prune`) once role checks
  exist; today they only require auth.
- `[PLANNED]` API access tokens / PATs for programmatic API + Swagger UI use (alongside the
  JWT login flow).
- `[PLANNED]` Reenact-run endpoint: re-run a past run from its stored `config_snapshot`
  (distinct from ad-hoc run, which runs the job's *current* definition on demand).
- `[PLANNED]` Ad-hoc run scheduled for a future datetime (a one-off, not "run now").
- `[PLANNED]` Graceful shutdown: on SIGTERM stop claiming and drain/await (or requeue)
  in-flight runs before exit.

## 9. UI (no changes yet; queued)

- `[PLANNED]` Live log streaming view + run detail.
- `[PLANNED]` Job duration graphs; per-worker run listing / worker detail.
- `[PLANNED]` Import/export of job definitions.
- `[PLANNED]` Surface prearm/armed status and misfire/retry config once those land.
- `[PLANNED]` Retention: a settings control for the window (`run_retention_days` /
  `prune_interval_secs`) and a manual "prune now" action. Backend is ready:
  `POST /api/v1/runs/prune?older_than_days=N` returns the count pruned.
- `[PLANNED]` Dashboard: runs grouped by job, plus load-more/pagination and incremental
  polling (only fetch new/updated runs).
- `[DONE]` Tenant management UI (`web-ui` `/tenants`, admin-only nav): list tenants;
  system admins create them. `[PLANNED]` Tenant context/picker (needs a backend
  scope-override; today listings derive scope from the JWT only).
- `[DONE]` Users dashboard for the admin role (`web-ui` `/users`, admin-only nav): list,
  create (with role + tenant for system admins), edit (username/password/role), delete
  (self-delete guarded). Tenant is fixed after creation.
- `[DONE]` Shared DB-config UI (`web-ui` `/db-configs`): create/edit/delete pgsql/mysql
  configs; the password field picks an existing secret (stored as a `secret:<name>` ref).
- `[DONE]` Job runner-config UI: JobForm covers all six runner types (shell/http/pgsql/
  mysql/python/node) with per-type fields, a key/value env editor (values may be
  `secret:<name>`), and the db runners pick a shared DB config. The job slide-over is the
  wide variant; the form is sectioned (Schedule/Runner/Environment/Execution). JobDetail
  renders the runner per-type and the env. `[PLANNED]` SSH configs and a "test config"
  action (§3); a richer retry editor (form sends no retry override yet).
- `[DONE]` Same-name job warning on create/edit (JobForm warns inline and confirms before
  saving a duplicate name).
- `[PLANNED]` Lint cleanup (e.g. the `react-refresh/only-export-components` disables).

## 10. CI / Docker follow-ups

- `[PLANNED]` Add a `/health` endpoint and a Dockerfile `HEALTHCHECK` (omitted until the
  route exists). (Note: an API `/health` route already exists; wire the Docker
  healthcheck to it.)
- `[PLANNED]` A tag/release workflow (publish versioned images/binaries on git tags).
- `[PLANNED]` Build and embed the web-ui SPA into the `arbiter-node` image.

## 11. Schema (`docker/init/000_schema.sql`)

- `[PLANNED]` Scheduled-vs-template jobs, retry/acceptance config, name uniqueness,
  HTTP-runner auth step, SSH prepare step, misfire-policy storage rework + constraints.
- `[PLANNED]` Unique constraint on job runner-config rows (one config per job per type).
- `[PLANNED]` Misfire storage rework: split the single `misfire_policy` string into
  `misfire_type` + `misfire_value` columns (value = the applicable time window). Track for
  now, do when it fits.

## 12. Runtime (admin-settable) settings

- `[DONE]` Settings store: a `settings` table (key/value) on both backends,
  `SettingsStore` (get/set/list) on the `Store` trait, conformance `settings` group.
  The shared DB is the cluster-replication substrate.
- `[DONE]` Typed, auto-refreshing wrapper: `core::RuntimeSettings` caches the settings
  snapshot and hands out typed accessors (`misfire_catchup_secs()`, `run_retention_secs()`,
  `prune_interval_secs()`) that fall back to static-config defaults. The scheduler and
  worker call those instead of stringly-typed `get_setting`. The cache refreshes on a
  change notification with a periodic poll backstop (no stale-until-restart, low DB load).
- `[DONE]` Change notification: `SettingsStore::await_settings_change` (default never
  fires = poll-only). Postgres uses `LISTEN`/`NOTIFY` (`set_setting` issues `pg_notify`);
  single-node SQLite uses an in-process `tokio::sync::Notify`. Best-effort by design — the
  backstop poll is the correctness guarantee, so a missed notify only adds bounded lag.
- `[PLANNED]` Migrate the remaining knobs to settings (worker `capacity`,
  tick/heartbeat intervals, `dead_after_secs`) via the same `RuntimeSettings` pattern.
- `[PLANNED]` Key validation/whitelist + typed coercion; role-gate the write endpoint.
- `[PLANNED]` UI: a settings panel to view/edit (backend ready).
- `[DONE]` Worker run-claiming wakes on a notification: `RunStore::await_runs_change`
  (channel `arbiter_runs`) is fired by `insert_job_run_if_missing` (scheduler
  materialize), `create_adhoc_run` (run-now), and `reschedule_for_retry`. The worker loop
  `select!`s it against the jittered tick poll (the backstop), so a due run is claimed
  within ms instead of waiting out a tick; claiming stays race-safe across workers
  (`FOR UPDATE SKIP LOCKED`). Verified live: run-now executed end to end in ~30ms. This
  closes the scheduler->worker handoff (materialize -> wake -> claim) event-driven end to
  end.
- `[DONE]` Worker plans to the next due time instead of polling: the claim loop sleeps
  until `RunStore::next_claimable_at` (earliest queued run for an enabled job), capped by
  `worker.claim_backstop_secs` (RuntimeSettings; default 300, `0` = unbounded), and wakes
  on the runs notification. Heartbeat + dead-worker reclaim moved to their own task so the
  claim loop can idle for minutes without affecting liveness. Sleeps are floored at the
  config tick so an overdue-but-unclaimable run (job at max concurrency) cannot spin.
  Conformance `claim::next_claimable_at_earliest_enabled` (both backends).
- `[DONE]` Backstop jitter: `core::jittered_backstop_secs(base, pct)` jitters the idle
  backstop downward (within the ceiling) so independent scheduler/worker nodes do not wake
  in lockstep. Applied in both loops (15%); precise next-fire/next-due sleeps are untouched,
  so the deterministic loop tests stay stable.
- `[DONE]` Deterministic scheduler-loop tests: a `Clock` seam (`core::Clock` /
  `SystemClock`) lets the loops take an injectable clock (production passes `SystemClock`).
  `scheduler/tests/event_driven.rs` runs the real `run_scheduler_loop` against a mock store
  with a `VirtualClock` pinned to tokio's paused time, so advancing `tokio::time` moves
  "now" in lockstep - it asserts replan-on-notification and materialize-as-time-advances
  with zero real waiting.
- `[DONE]` Deterministic worker-loop tests (`worker/tests/worker_loop.rs`): the real
  `run_worker_loop` against a `Store` mock whose `claim_job_runs` reads the same
  `VirtualClock` for due-ness and returns empty (so nothing executes - it only records
  *when* a due run would be claimed). Asserts it claims right at the due time (not before -
  plan-to-next-due) and wakes on a runs notification without waiting out the backstop.
  Unused `Store` methods are `unimplemented!()` and never reached on this path.
- `[DONE]` Event-driven scheduler (replaced the fixed tick): the leader materializes
  due/imminent fires then sleeps until the next un-materialized fire approaches, capped by
  `scheduler.backstop_secs` (RuntimeSettings; default 180, `0` = unbounded). It replans
  early on a `JobStore::await_jobs_change` notification — create/update/enable/disable/
  delete fire it (PG `LISTEN`/`NOTIFY` channel `arbiter_jobs`, in-process Notify on SQLite).
  Followers keep a short fixed leadership-check cadence so failover stays fast. Wake =
  min(next_fire - lookahead, now + backstop). Verified live: enable -> run materialized in
  ~30ms. The lookahead window (60s) doubles as the materialize lead. `[PLANNED]` apply the
  same to worker run-claiming (channel `arbiter_runs`).
- `[PLANNED]` Per-node config from the DB, read live (like settings) via the same wrapper.

## 13. Secrets (plan before DB runners)

Design home: **`SECRETS.md`** (threat model + key architecture: per-node-keypair envelope,
DB-as-sealed-transport, versioned keyring; distributed KEK rotation state machine with
pseudocode; quantum note). The bullets below are the tracker; `SECRETS.md` is the spec.

DB runners carry a connection password. Today `pgsql_configs.password_secret` is
plaintext and `build_snapshot` resolves it *into* `config_snapshot` -- so a plaintext
password would be persisted in `job_runs.config_snapshot`. Plan:

- `[PLANNED]` **Define "secure" first (threat model) -- do this before any design.** Enumerate
  adversaries and, for each surface, what may be visible and in what manner that is
  *acceptable*: at-rest (DB rows, backups/dumps), in-transit, in-memory, process listing
  (`ps`/`/proc/<pid>/cmdline` -- argv) and environment (`/proc/<pid>/environ` -- env,
  inherited by grandchildren), logs/traces, persisted `config_snapshot`, and the admin API.
  Likely stance: secret *names/ids* and *metadata* may be visible to operators; plaintext
  *values* must never appear at rest, in logs, in `ps`/env, or in snapshots; the box's root
  user is out of scope (trusted). The highest-sensitivity surface -- and the focus of the
  model -- is the secrets subsystem itself: the **master key material** and the **rotation
  flow**, since that is where keys and plaintext are actually handled. The recurring
  "doesn't feel secure/easy enough" worry should be resolved against this written model,
  not ad hoc.
- `[PLANNED]` Resolve at execution, not at snapshot-build: the snapshot carries a secret
  *reference* (id); the worker resolves the value just before connecting. Keeps plaintext
  out of persisted snapshots. (Matches the core "Secret type holds id only, resolved at
  the last moment" TODO.) Same convention extends to job **env vars**: a value may be a
  secret reference (e.g. `secret:<id>`) resolved by the worker at execution -- the snapshot
  stores the reference, never plaintext. (`set_job_env` already lands env; the resolver is
  the future piece.)
- `[PLANNED]` Encrypt at rest: a `secrets` table (id, name, ciphertext, timestamps) + a
  `SecretStore` (set / resolve / list-without-values). Symmetric encryption with a master
  key from config/env (single-binary friendly); pluggable external managers (Vault/KMS)
  later; key rotation tracked separately.
- `[DONE]` Configs reference a secret (not the value): `pgsql_configs`/`mysql_configs`
  `password_secret` holds a `secret:<name>` reference, resolved by the worker at execution.
  Shared-config CRUD is a `ConfigStore` (create/get/list/update/delete, soft-delete,
  tenant-scoped) on both backends (SQLite gained the config tables) with conformance
  `config::*`, and a tenant-scoped API: `POST`/`GET /api/v1/db-configs`,
  `GET`/`PATCH`/`DELETE /api/v1/db-configs/{id}`.
- `[DONE]` API: write-only secret endpoints — `POST /api/v1/secrets` (create, value
  encrypted via `AppState.secrets` SecretAdmin, never returned; 503 on a keyless node),
  `GET /api/v1/secrets` (metadata only), `DELETE /api/v1/secrets/{id}`. Tenant-scoped via
  the JWT. The no-plaintext rule (I4) is enforced by type: `SecretMetaResponse` has no
  value field. `[DONE]` UI: a secrets panel (`web-ui` `/secrets`) lists metadata and
  creates/deletes; values are write-only (a create form, never shown back).
  `[PLANNED]` Role-gate writes to Operator+ (today any authenticated caller in scope can
  write, matching the rest of the API).
- `[PLANNED]` HTTP auth and SSH configs should reuse the same secret store when they land.
- `[PLANNED]` Decisions to confirm: encryption crate (e.g. `chacha20poly1305`/`aes-gcm`),
  master-key source, and the enforcing conformance angle (assert resolved snapshots never
  embed plaintext secrets).
- `[PLANNED]` Key management + rotation is a **distributed** problem (not a scale one): every
  node must decrypt at execution, so use a **versioned keyring** (each secret tagged with the
  key version that encrypted it) with a **transition window** where nodes accept both old and
  new keys. Rotation = re-encrypt every secret to the new version, then retire the old key --
  but only once *every* secret is re-encrypted *and every node* has the new key (a
  coordination/barrier, natural fit for the cluster layer). The re-encrypt job must be
  batched, resumable, and idempotent -- which is exactly what makes it the ops-friendly
  "button + progress bar" experience (guiding light: easy/intuitive for ops, no weird
  rotation rituals). Envelope encryption (master key wraps per-secret/data keys) makes
  rotation cheaper (rewrap keys, not every value).

DB runner execution (`execute_pgsql_query` / `execute_mysql_query`) is done (worker), using
a secret reference resolved at execution. The secret and shared-config CRUD APIs are done
(secrets store `secret:<name>` refs end to end). What remains for the *end-to-end product*
flow is the secret + config UI.

## 14. Tenancy (see `TENANCY.md`)

Model is scope x level: scope = a user's `tenant_id` (`NULL` = system), level = role
(Admin/Operator/Viewer; the old `Tenant` role is gone).

- `[DONE]` Data model: `tenants` table + seeded default; `tenant_id` on jobs/secrets/configs
  (NOT NULL, default tenant) and users (nullable = system); `TenantStore`; `UserRole` loses
  `Tenant`.
- `[DONE]` Scoping: `create_job`/`upsert_secret` stamp the tenant; `list_jobs`/`get_job`/
  `list_recent_runs` + secret reads filter by scope; secrets unique per tenant; JWT carries
  `tenant_id`, handlers derive scope and gate job mutations. Conformance `tenant::*`,
  `secrets::isolated_per_tenant`.
- `[DONE]` Secret tenant isolation (SECRETS.md I7): the worker resolves a run's secrets in
  its job's tenant, fail closed.
- `[PLANNED]` Remaining: `cancel_run` tenant scoping (keyed by run id, needs the run's
  tenant); create-user API sets the new user's tenant; tenant management + picker UI
  (increment 4); per-tenant quotas later.

## Resolved (kept for the record)

- `[DONE]` store-pg `update_user` reversed `COALESCE` — `COALESCE(col, $n)` kept the old
  value, silently dropping updates. Fixed to `COALESCE($n, col)`. Caught by the
  `crud::update_user` conformance case.
