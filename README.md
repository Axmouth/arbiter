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
  `utoipa`, and it serves the web UI.
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

You'll need Rust, Node 20+ (or 22+), and a running Postgres.

```bash
# REST API + embedded web UI
cargo run -p arbiter-api

# a scheduler/worker node
cargo run -p arbiter-node
```

The API listens on `:8080`, with Swagger at `/swagger-ui`. You can run as many
`arbiter-node` processes as you like — they share the queue, and only one acts as the
scheduler at any moment.

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
the working directory, `/etc/arbiter/`, and your home config dir). The env var worth
knowing:

- `ARBITER_ALLOW_MULTI_ID` — allow multiple worker processes to share a machine with
  separate identities. Handy for local testing; off by default.

## What works, what doesn't

Working today: cron scheduling, run materialization and claiming, the shell runner,
run history, worker registration with heartbeats and offline detection, scheduler
leader election, the REST API and OpenAPI docs, and the dashboard (job CRUD,
enable/disable, run-now, run history, worker status).

Not yet, or only partial:

- Runners other than shell (HTTP, Postgres/MySQL, Python, Node) are modeled in the
  schema but not executed yet.
- Misfire policies and per-job max-concurrency are defined but not fully enforced.
- Job timeouts, tags/grouping, persisted logs, duration graphs, import/export.
- Shared runner configs (DB credentials, HTTP auth, SSH) — designed, not built.

## Inspiration

Cronicle for the overall shape, Quartz and StackStorm for scheduling ideas, Temporal
for the durability mindset. Arbiter takes the parts that fit and tries to stay small.
