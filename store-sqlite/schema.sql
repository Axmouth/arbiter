-- SQLite schema for the embedded arbiter backend.
-- Used both at runtime (applied on connect) and to build the compile-time
-- query-check database for sqlx's offline cache.

CREATE TABLE IF NOT EXISTS workers (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    hostname TEXT NOT NULL,
    last_seen TEXT NOT NULL,
    capacity INTEGER NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    restart_count INTEGER NOT NULL DEFAULT 0,
    version TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    schedule_cron TEXT,
    enabled INTEGER NOT NULL DEFAULT 0,
    runner_type TEXT NOT NULL,
    max_concurrency INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    misfire_policy TEXT NOT NULL DEFAULT 'run_immediately',
    deleted_at TEXT
);

CREATE TABLE IF NOT EXISTS job_runner_shell (
    job_id TEXT PRIMARY KEY,
    command TEXT NOT NULL,
    working_dir TEXT
);

CREATE TABLE IF NOT EXISTS job_runner_http (
    job_id TEXT PRIMARY KEY,
    method TEXT NOT NULL,
    url TEXT NOT NULL,
    headers TEXT,
    body TEXT,
    timeout_sec INTEGER
);

CREATE TABLE IF NOT EXISTS job_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    scheduled_for TEXT NOT NULL,
    state TEXT NOT NULL,
    worker_id TEXT,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    output TEXT,
    error_output TEXT,
    config_snapshot TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS job_runs_unique_run ON job_runs(job_id, scheduled_for);

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS leader_lease (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    holder TEXT,
    expires_at TEXT
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
