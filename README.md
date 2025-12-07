
# 📘 Dromio - Distributed Job Scheduler (Rust + Axum + React)

**Dromio** is a distributed, Cronicle-inspired job scheduler written in **Rust**, featuring:

* A clean **REST API** (with OpenAPI/Swagger docs generated via *utoipa*)
* A modern **web dashboard** (React, Tailwind, TanStack Query/Router)
* Pluggable **workers** that execute scheduled and ad-hoc tasks
* Persistent storage for jobs, runs, and worker metadata
* Human-friendly cron editor + run history + job inspection

It is designed to be simple, reliable, and easy to extend — without needing Node or the original Cronicle runtime.

---

## ✨ Features

### 🧠 Scheduler

* Cron-based job scheduling (validated via `croner`)
* Misfire policy support (run immediately, skip, coalesce, run all, run if late, etc.) (WIP)
* Max concurrency limits per job (WIP)
* Ad-hoc execution (“Run Now”)

### 🧵 Worker Model

* Multiple workers can register and pick up tasks
* Each worker reports:

  * `hostname`
  * `capacity`
  * `last_seen`
* Automatic offline detection

### 📊 Web Dashboard

Modern UI built with:

* **React**
* **TanStack Router**
* **TanStack Query**
* **TailwindCSS**
* **HeadlessUI** slide-over panels
* **react-js-cron** visual cron builder
* Human-readable cron text (`cronstrue`)
* Live-updating run history per job
* Workers status page (online/offline, last heartbeat)
* Full CRUD for jobs:

  * Create job
  * Edit job
  * Delete job
  * Enable/disable job
  * Run job immediately

### 🔌 API

REST API exposed under `/api/v1`, including:

* `POST /jobs`

* `GET /jobs`

* `PATCH /jobs/:id`

* `DELETE /jobs/:id`

* `POST /jobs/:id/enable`

* `POST /jobs/:id/disable`

* `POST /jobs/:id/run`

* `GET /runs`

* `GET /runs?job_id=...`

* `POST /runs/:id/cancel`

* `GET /workers`

Includes **OpenAPI/Swagger UI** via `utoipa`.

### 🧬 Type-safe front-end integration

The frontend types are generated from Rust structs using **ts-rs**, so the API and UI stay in sync automatically.

---

## 🚀 Getting Started

### Prerequisites

* Rust 1.70+
* Node.js 20.12+ **or** 22.12+
* PostgreSQL

---

## 🛠 Backend

### Run the API server

```bash
cd dromio
cargo run --bin dromio-api
```

This starts:

* The scheduler
* Worker heartbeat tracking
* HTTP API on port `8080`
* The embedded SPA (web UI) in production mode (WIP)

Open:

```
http://localhost:8080
```

Swagger/OpenAPI:

```
http://localhost:8080/swagger-ui
```

---

## 🧵 Worker

Run a worker node:

```bash
cargo run --bin dromio-node
```

Workers:

* Poll the scheduler
* Execute jobs
* Report state transitions (queued -> running -> succeeded/failed)
* Send heartbeats

You can run one or many workers - Dromio is horizontally scalable.

---

## 🌐 Web UI (Dev Mode)

```bash
cd web-ui
npm install
npm run dev
```

This runs the React UI with hot reload at:

```
http://localhost:5173
```

The backend continues to serve `/api/v1`, so the UI communicates with your Rust API normally.

---

## 🗂 Project Structure

```
dromio/
  ├── dromio-core/          # Scheduler logic, cron, models, store traits
  ├── dromio-api/           # Axum API server + embedded SPA
  ├── dromio-worker/        # General Worker functionality
  ├── dromio-node/          # Node process implementation
  └── web-ui/               # React dashboard (ts-rs generated types)
```

---

## 🔧 Configuration

Environment variables (planned / partial):

| Name               | Default | Description                        |
| ------------------ | ------- | ---------------------------------- |
| `DROMIO_ALLOW_MULTI_ID` | 0 | Allows multiple worker IDs per container/server (mostly used for local development) |

---

## 🧩 Roadmap

* [x] CRUD jobs
* [x] Enable / disable jobs
* [x] Run Now
* [x] Job history view
* [x] Worker status dashboard
* [x] Cron builder (react-js-cron)
* [x] Human readable cron text
* [x] Soft delete jobs
* [x] `job_id` filtering for runs
* [x] `ts-rs` for type sharing
* [x] OpenAPI UI (utoipa)
* [x] Leader election for scheduler
* [x] Persistent worker identities
* [x] Jitter for scheduling/claiming to avoid thundering herd
* [x] Dark mode on UI
* [ ] Misfire policy implementation
* [ ] Job tags / grouping
* [ ] Logs storage + UI
* [x] Error messages for failed runs
* [ ] Job duration graphs
* [ ] Worker detail view + per-worker run listing
* [x] Authentication / roles (admin/operator)
* [ ] Import/export job definitions
* [x] Different runners(process, http, database, python, etc)
* [ ] Shared config to be used for runners(DB credentials, http auth, SSH config)
* [ ] Explore real-time SSE/Websocket run updates or other ways to increase efficiency

---

## Inspiration

Dromio is inspired by:

* **Cronicle**
* **StackStorm** scheduling
* **Temporal** workflows
* **Quartz**

But written from scratch in **Rust**, with modern DX + improved UI.

---
