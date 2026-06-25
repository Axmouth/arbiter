# Arbiter

A distributed job scheduler written in Rust. You define jobs with cron schedules, and
a pool of workers picks up the due runs and executes them — shell commands today, with
HTTP, database, and script runners on the way. There's a REST API (with OpenAPI docs)
and a React dashboard for managing jobs and watching runs go by.

It's inspired by Cronicle, Quartz, and the scheduling side of StackStorm/Temporal, but
it's a from-scratch implementation aimed at being small, predictable, and easy to run.

> **Status:** pre-release and actively changing. The core scheduling loop, worker
> execution, and the API/UI all work; several things listed below are partial or
> stubbed. Treat it as a work in progress, not something to put in front of
> production yet.

## How it works

Arbiter is split into a handful of crates:

- **`arbiter-scheduler`** — the brain. Each tick it works out which job runs are due
  and materializes them. Only one scheduler is active at a time (leader-elected via a
  Postgres advisory lock), so you can run several nodes without double-scheduling.
- **`arbiter-worker`** — the muscle. Workers heartbeat, claim due runs up to their
  capacity, execute them, and report results. Claiming uses `FOR UPDATE SKIP LOCKED`,
  so many workers share one queue without stepping on each other.
- **`arbiter-api`** — an Axum REST API under `/api/v1` with Swagger/OpenAPI via
  `utoipa`, and it serves the web UI. It is a library mounted by a node running the
  `api` role, not a separate process.
- **`arbiter-core`** — the domain model, the cron logic (`croner`), and the `Store`
  trait everything is built on.
- **`arbiter-store-pg`** — the Postgres implementation of `Store`.
- **`web-ui`** — a React + Tailwind dashboard (TanStack Router/Query). The frontend
  types are generated from the Rust structs with `ts-rs`, so the API and UI don't
  drift apart.

A run moves through `queued -> running -> succeeded | failed | cancelled`. Jobs and
run history live in Postgres. Each worker keeps a persistent identity (a UUID on disk,
file-locked) so restarts are tracked rather than spawning duplicate workers.

### Storage backends

Everything the scheduler and workers touch goes through the `Store` trait, which keeps
the door open for backends beyond Postgres. The direction — single-node SQLite for
solo/single-box setups, Postgres for scaling workers across machines, and an
embeddable, consensus-coordinated option for HA without external services — is written
up in:

- [BACKEND_ARCHITECTURE.md](BACKEND_ARCHITECTURE.md) — the backend options, a taxonomy
  of what data lives where, and the scheduling models that fit each.
- [BACKEND_CONFORMANCE_TESTS.md](BACKEND_CONFORMANCE_TESTS.md) — one test suite that
  grades any backend against the same behavioral contract.

Postgres is the only implemented backend today.

## Getting started

You'll need Rust, Node 20+ (or 22+), and Docker (for Postgres).

The fastest path is the dev script — it starts Postgres in Docker, builds the web UI,
and runs an all-in-one node:

```bash
./dev.sh                 # then open http://localhost:8080, log in admin / admin
./dev.sh --skip-ui       # reuse the existing ui_dist for faster restarts
ARBITER_API_PORT=8090 ./dev.sh   # if 8080 is taken
```

Or run the pieces yourself against your own Postgres:

```bash
# an all-in-one node: API + scheduler + worker (default roles)
cargo run -p arbiter-node
```

`arbiter-node` is the single binary for the whole system. It is a cluster member with
its own identity that runs any subset of three roles — `api`, `scheduler`, `worker` —
toggled by config. All-on is the single-node default; split them to compose a cluster
(an api-only control plane, a worker-only fleet). For example:

```bash
# a worker-only node (no API, no scheduler)
ARBITER_ROLES_API=false ARBITER_ROLES_SCHEDULER=false cargo run -p arbiter-node
```

The API listens on `:8080`, with Swagger at `/swagger-ui`. You can run as many nodes
as you like — they share the queue, and only one scheduler is active at any moment.

For UI development with hot reload:

```bash
cd web-ui
npm install
npm run dev
```

That serves the dashboard on `:5173`, talking to the Rust API.

## Configuration

Config comes from a TOML file plus a couple of environment variables. Copy
`config/arbiter.example.toml` to `config/arbiter.toml` (the loader searches there,
the working directory, `/etc/arbiter/`, and your home config dir). Any setting also
takes an env override (`ARBITER_` prefix, `_` between sections, e.g.
`ARBITER_DATABASE_URL`, `ARBITER_API_PORT`). The env vars worth knowing:

- `ARBITER_ROLES_API`, `ARBITER_ROLES_SCHEDULER`, `ARBITER_ROLES_WORKER` — toggle which
  roles this node runs (all on by default). See `[roles]` in the example config.
- `ARBITER_ALLOW_MULTI_ID` — allow multiple worker processes to share a machine with
  separate identities. Handy for local testing; off by default.
- `ARBITER_DATA_DIR` — where the node persists its identity file (default `/data`). Set it
  to a writable local path (e.g. `./.dev/data`) when running without root.

## What works, what doesn't

For the full inventory see [IMPLEMENTED_SURFACE.md](IMPLEMENTED_SURFACE.md) (a "reverse
roadmap" of everything built). In short:

Working today: cron scheduling (event-driven), run materialization/claiming with retry and
structured outcomes, all six runners (shell, HTTP, Postgres, MySQL, Python, Node), per-job
env vars, encrypted secrets with multi-node KEK distribution and ack-barrier rotation, shared
DB configs, tenancy (scope x level), runtime settings, worker heartbeats and dead-worker
reclaim, scheduler leader election, both store backends (Postgres + SQLite) under one
conformance suite, the REST API + OpenAPI, and the dashboard (jobs, runs, workers, secrets,
db-configs, tenants, users, keyholders with live rotation progress).

Not yet, or only partial:

- Clustered durability/HA strategies (see [BACKEND_ARCHITECTURE.md](BACKEND_ARCHITECTURE.md)).
- Job timeouts, tags/grouping, persisted/streamed logs, duration graphs, import/export.
- `cancel_run` tenant scoping; broader SSE adoption; a Go client.

See [FOLLOWUPS.md](FOLLOWUPS.md) for the live list.

## Inspiration

Cronicle for the overall shape, Quartz and StackStorm for scheduling ideas, Temporal
for the durability mindset. Arbiter takes the parts that fit and tries to stay small.
