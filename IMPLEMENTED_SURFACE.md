# Implemented surface (reverse roadmap)

What arbiter actually does today, by area. This is the inverse of a roadmap: instead of what
is planned, it inventories what is built and reachable. Forward-looking work lives in
[FOLLOWUPS.md](FOLLOWUPS.md); deep design rationale in
[BACKEND_ARCHITECTURE.md](BACKEND_ARCHITECTURE.md), [SECRETS.md](SECRETS.md),
[TENANCY.md](TENANCY.md), [RUNNER_RESULT_PROTOCOL.md](RUNNER_RESULT_PROTOCOL.md), and the
rotation walkthrough in [KEK_ROTATION.md](KEK_ROTATION.md).

Keep this current: any change that adds or removes a user-visible surface (an endpoint, a
runner, a store method, a UI page, a config knob) updates this file in the same commit.

## Topology

- **One role-composed binary** `arbiter-node`. A node is a cluster member with its own
  crypto identity that runs any subset of `api` / `scheduler` / `worker` roles, toggled by
  `[roles]` config (env `ARBITER_ROLES_API/SCHEDULER/WORKER`, default all on). The api crate
  is a library (`arbiter_api::run_http_api`); there is no separate api binary.
- **Workspace crates:** `core` (model + traits), `crypto`, `secrets`, `store-pg`,
  `store-sqlite`, `store-tests` (shared conformance suite), `scheduler`, `worker`, `api`,
  `config`, `node`, `cli`.
- Docker image ships only `arbiter-node`; compose has api-only and worker-only examples.

## Storage backends

- **`Store` supertrait** = `ApiStore + JobStore + RunStore + WorkerStore + SettingsStore +
  SecretStore + TenantStore + ConfigStore`. Two implementations, behind one capability-gated
  conformance suite (`arbiter-store-tests`, **136 cases**, run per `backend::group::case`):
  - **Postgres** (`store-pg`) — `LISTEN`/`NOTIFY`, advisory-lock-free leader lease,
    `FOR UPDATE SKIP LOCKED` claim.
  - **SQLite** (`store-sqlite`) — embedded, in-process `tokio::sync::Notify`, Uuid stored as
    16-byte blob.
- Compile-time `sqlx::query!` with committed `.sqlx` offline caches per crate.

## Jobs and scheduling

- Cron jobs (`croner`), per-job enable/disable, ad-hoc "run now".
- **Event-driven scheduler:** leader materializes due/imminent fires (60s lookahead), plans
  to the next un-materialized fire, sleeps until then capped by a configurable backstop
  (default 180s, jittered), and replans immediately on a job-change notification. Followers
  keep a short leadership-check cadence for fast failover.
- **Misfire policies** with self-bounded windows (`misfire_catchup_secs` caps only unbounded
  policies).
- Versioned `partition_count` plumbing and version-parameterized routing are present for
  forward-compatible repartitioning (fixed-at-create today).

## Runners

Six runner kinds, all end to end on both backends, executed from a config snapshot persisted
at claim time:

- **shell** — command + args + env.
- **http** — method/url/headers/body; retryable on 408/425/429/5xx + transport.
- **pgsql** / **mysql** — execute a query via sqlx; password is a `secret:<name>` reference
  resolved at execution; classifies success / failed / retryable (conn/timeout).
- **python** / **node** — injected stdlib-only language runtime owns the transport; user code
  is `run(ctx) -> X` (+ optional `prepare(ctx)`). Structured result protocol v1
  (`{protocolVersion, status, output, error}`) over a result file.
- Per-job **env vars** (`GET`/`PUT /jobs/{id}/env`, plus on create/update); values may be
  `secret:<name>`.

## Runs, outcomes, retry

- Worker capacity (RAII counter), heartbeat + dead-worker reclaim on a dedicated task.
- **Plan-to-next-due** claim loop: sleeps until the earliest claimable run capped by a
  configurable worker backstop (default 300s, jittered), wakes on a run-change notification.
- Structured outcomes: `stdout`/`stderr` text streams, typed `result` + `result_media_type`,
  `error` + `error_media_type`, `result_status` (success | failed | retryable), `attempt`.
- **Live output:** subprocess runners (shell/python/node) stream captured stdout/stderr to
  the run row as they execute (chunk-based, flushed ~every 500ms via `update_run_output`,
  which fires the runs notify), so a run view shows output grow in real time. Claim and
  finalize also fire the notify, so the whole queued -> running -> done lifecycle pushes.
- **Per-job retry:** `max_attempts` + `backoff_strategy` (fixed | exponential | fibonacci)
  with base/cap and mandatory full jitter.
- Run **retention** (`prune_runs` + worker prune loop + API).

## Secrets and KEK rotation

- Three-layer envelope: value -> per-secret DEK -> versioned KEK (memory only) -> sealed to
  each node's X25519 public key in the DB. Crypto: XChaCha20-Poly1305 + X25519 sealed boxes
  (`arbiter-crypto`, traits for FIPS/hybrid later). Node identity is a versioned keyring in a
  0600 file behind `NodeIdentityStore`.
- **Single-node:** auto-bootstrap on first run; set / resolve work end to end. Values are
  write-only over the API (I4, enforced by type — no value field on any response).
- **Multi-node:** join registers a `pending` public key; admin approves; a KEK-holder's
  reconcile seals the active KEK to approved nodes; `refresh_keyring` lets a running node pick
  up newly sealed versions without a restart.
- **KEK rotation (ack-barrier, observable):** publish -> ack barrier -> re-wrap -> retire;
  version lifecycle `pending -> active`, old `active -> retiring -> retired`. Driven
  idempotently by every node's KEK task, so a cluster rotation self-completes. Revoke +
  rotate fully locks a node out. Evict drops a dead node from the barrier. Live progress via
  `core::rotation_status` over SSE. Full walkthrough: [KEK_ROTATION.md](KEK_ROTATION.md).
- Runner integration: `secret:<name>` references resolved at execution for env vars and DB
  passwords, within the run's tenant (fail closed).

## Tenancy

- Model is **scope x level**: scope = `user.tenant_id` (NULL = system, Some = tenant), level
  = role (Admin / Operator / Viewer). `tenants` table + seeded `default` tenant.
- `tenant_id` on jobs / runs / secrets / db-configs; enforced end to end (list/get/create
  filter by scope; cross-tenant get returns 404; secret isolation I7). JWT claims carry the
  tenant; system scope (None) sees all. Worker/scheduler run as system scope.
- Gaps: `cancel_run` is run-keyed and not yet tenant-scoped.

## Configuration

- **`arbiter-config`** is the sole owner of env reads; everything else takes typed config.
  Sections: admin, api, roles, node (data_dir/identity_path/allow_multi_id), retention,
  scheduler (misfire/backstop), worker (claim backstop). Existing env names still map.
- **Runtime settings:** `RuntimeSettings` is a cached typed view over `SettingsStore`, read
  live at use-time, refreshed on a change notification with a poll backstop. Surfaced as
  `GET`/`PUT /api/v1/settings`.

## Live updates / events

- **Notify-or-backstop** pattern on three channels (`arbiter_settings`, `arbiter_jobs`,
  `arbiter_runs`): per-store `await_*_change` + fire-on-mutation (PG `pg_notify`/`PgListener`,
  SQLite in-process `Notify`); the poll backstop is the correctness guarantee.
- **Server-Sent Events** (cookie-authed, the browser `EventSource` sends the session
  cookie): `GET /api/v1/secrets/rotation/stream` (live rotation progress) and
  `GET /api/v1/runs/stream` (a lightweight `change` ping on the `arbiter_runs` notify channel
  that the dashboard and job-detail history use to refetch on change instead of polling), and
  `GET /api/v1/runs/{id}/stream` (a payload stream of one run's state + live output that
  closes on terminal). Two reusable helpers in api `sse.rs`: `change_stream` (ping, paired
  with the `useChangeStream` hook) and `snapshot_stream` (payload, paired with `useRunStream`
  / the rotation progress). Adding a feed for another resource is a few lines. Remaining
  polling pages are SSE candidates (FOLLOWUPS).

## HTTP API

Base `/api/v1` (cookie-authed JWT, `AuthClaims` / `AdminRequired` extractors), auth under
`/api`.

- **Jobs:** `POST/GET /jobs`, `GET/PUT/DELETE /jobs/{id}`, `GET/PUT /jobs/{id}/env`,
  `POST /jobs/{id}/enable|disable`, `POST /jobs/{id}/run`.
- **Runs:** `GET /runs` (filters `byJobId`/`byWorkerId`, camelCase), `GET /runs/stream` (SSE
  change pings), `GET /runs/{id}`, `GET /runs/{id}/stream` (SSE payload stream of one run's
  state + output, closes on terminal), `POST /runs/{id}/cancel`, `POST /runs/prune`.
- **Settings:** `GET/PUT /settings`.
- **Workers:** `GET /workers`.
- **Secrets:** `POST/GET /secrets`, `DELETE /secrets/{id}`, `POST /secrets/rotate`,
  `GET /secrets/rotation`, `GET /secrets/rotation/stream` (SSE).
- **Tenants:** `POST/GET /tenants`.
- **DB configs:** `POST/GET /db-configs`, `GET/PATCH/DELETE /db-configs/{id}`.
- **Node keys:** `GET /node-keys`, `POST /node-keys/{id}/approve|revoke`,
  `DELETE /node-keys/{id}` (evict). System-admin only.
- **Auth/users (`/api`):** `POST /login`, `POST /logout`, `GET /me`, `GET /users`,
  `GET/POST/PUT/DELETE /users/...`.
- `GET /health`, OpenAPI + Swagger UI at `/swagger-ui`.

## Web UI

React + Tailwind (TanStack Router/Query), ts-rs bindings, built into tracked `ui_dist/` and
served by the api role. Pages: Home, Login, Jobs, JobDetail, Runs (grouped-by-job +
load-more), RunDetail, Workers, Secrets, DbConfigs, Tenants, Users, Keyholders (approve /
revoke / evict + live rotation progress). Job form covers all six runner types with a
key/value env editor.

## Testing

- `arbiter-store-tests` — 134 backend conformance cases (both backends).
- Deterministic scheduler + worker loop tests over mock stores with a virtual clock on tokio
  paused time.
- `secrets` unit tests (15, incl. rotation barrier + lockout), `crypto` tests, worker
  full-flow (incl. retry + secret resolution) and live PG-gated DB-runner tests.
- CI + Dockerfile (ghcr).

## Not yet (pointers)

Clustered durability/HA strategies (BACKEND_ARCHITECTURE.md), `cancel_run` tenant scoping,
chunked transaction-backed re-wrap for very large secret sets, broader SSE adoption, Go
client. See FOLLOWUPS.md for the live list.
