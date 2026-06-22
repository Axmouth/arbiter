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

- `[PLANNED]` Node-management endpoints + a cluster-join protocol (`api/src/main.rs`).
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
- `[PLANNED]` Users dashboard for the admin role (user CRUD API already exists).
- `[PLANNED]` Config UI for non-shell runners (http/pgsql/mysql/python/node) and shared
  configs (DB credentials, SSH), with a "test config" action (§3).
- `[PLANNED]` Same-name job warning on create/edit.
- `[PLANNED]` Lint cleanup (e.g. the `react-refresh/only-export-components` disables).

## 10. CI / Docker follow-ups

- `[PLANNED]` Add a `/health` endpoint and a Dockerfile `HEALTHCHECK` (omitted until the
  route exists). (Note: an API `/health` route already exists; wire the Docker
  healthcheck to it.)
- `[PLANNED]` A tag/release workflow (publish versioned images/binaries on git tags).
- `[PLANNED]` Build and embed the web-ui SPA into the `arbiter-api` image.

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
- `[DONE]` Live overrides for the knobs shipped so far, each falling back to the static
  config default: the scheduler reads `scheduler.misfire_catchup_secs` live (leader
  only), the worker reads `retention.run_retention_days` / `retention.prune_interval_secs`
  live. Admin API: `GET /api/v1/settings`, `PUT /api/v1/settings/{key}`.
- `[PLANNED]` Migrate the remaining knobs to settings (worker `capacity`,
  tick/heartbeat intervals, `dead_after_secs`) via the same read-live pattern.
- `[PLANNED]` Key validation/whitelist + typed coercion; role-gate the write endpoint.
- `[PLANNED]` UI: a settings panel to view/edit (backend ready).
- `[PLANNED]` Per-node config from the DB, read live (like settings). Cut polling: poll
  rarely and react to change notifications (an in-process channel for single-node SQLite,
  Postgres `LISTEN`/`NOTIFY` for multi-node), and jitter poll intervals to desync workers
  and avoid congestion.

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
- `[PLANNED]` Configs reference a secret by id (stop storing the value); `pgsql_configs`/
  `mysql_configs` `password_secret` becomes a secret reference.
- `[PLANNED]` API: write-only secret endpoints (create/update/list metadata, never return
  plaintext). UI: a secrets panel.
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
a secret reference resolved at execution. What remains for the *end-to-end product* flow:
shared-config CRUD that stores `password_secret` as a `secret:<name>` reference, and the
secret create/list API + UI.

## 14. Tenancy (plan + implement soon)

Today only a `Tenant` user *role* exists (plus TODO filters in `api/src/routes.rs`); there
is no real multi-tenancy -- no `tenant_id` on jobs/runs/secrets/configs, no scoping.

- `[PLANNED]` Plan the tenancy model: `tenant_id` on jobs (and derived onto runs), secrets,
  shared configs, env vars; how Tenant users map to a tenant; admin/operator cross-tenant
  visibility vs Tenant-scoped views.
- `[PLANNED]` Enforce scoping in queries (list/get jobs/runs/secrets filtered by the
  caller's tenant) and on writes.
- `[PLANNED]` **Secret tenant isolation (SECRETS.md I7):** `resolve_secret` must check the
  requesting job's tenant against the secret's tenant and refuse a mismatch (fail closed).
  This depends on `tenant_id` existing on both jobs and secrets.

## Resolved (kept for the record)

- `[DONE]` store-pg `update_user` reversed `COALESCE` — `COALESCE(col, $n)` kept the old
  value, silently dropping updates. Fixed to `COALESCE($n, col)`. Caught by the
  `crud::update_user` conformance case.
