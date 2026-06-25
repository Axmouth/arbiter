-- SQLite schema for the embedded arbiter backend.
-- Used both at runtime (applied on connect) and to build the compile-time
-- query-check database for sqlx's offline cache.

-- Tenancy (see TENANCY.md). The default tenant owns rows created without an explicit one.
CREATE TABLE IF NOT EXISTS tenants (
    id TEXT PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    created_at TEXT NOT NULL
);

-- id is the default-tenant UUID as a 16-byte blob (sqlx stores Uuid as a blob here).
INSERT OR IGNORE INTO tenants (id, name, created_at)
VALUES (x'00000000000000000000000000000001', 'default', '1970-01-01T00:00:00+00:00');

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
    max_attempts INTEGER NOT NULL DEFAULT 1,
    backoff_strategy TEXT NOT NULL DEFAULT 'exponential',
    backoff_base_secs INTEGER NOT NULL DEFAULT 30,
    backoff_cap_secs INTEGER NOT NULL DEFAULT 3600,
    tenant_id BLOB NOT NULL DEFAULT x'00000000000000000000000000000001',
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

CREATE TABLE IF NOT EXISTS job_runner_python (
    job_id TEXT PRIMARY KEY,
    module TEXT NOT NULL,
    class_name TEXT NOT NULL,
    timeout_sec INTEGER
);

CREATE TABLE IF NOT EXISTS job_runner_node (
    job_id TEXT PRIMARY KEY,
    module TEXT NOT NULL,
    function_name TEXT NOT NULL,
    timeout_sec INTEGER
);

-- Per-job environment variables, injected into subprocess runners (shell/python/node).
CREATE TABLE IF NOT EXISTS job_env_vars (
    job_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (job_id, key)
);

CREATE TABLE IF NOT EXISTS job_runs (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL,
    scheduled_for TEXT NOT NULL,
    state TEXT NOT NULL,
    worker_id TEXT,
    attempt INTEGER NOT NULL DEFAULT 1,
    started_at TEXT,
    finished_at TEXT,
    exit_code INTEGER,
    result_status TEXT,
    stdout TEXT,
    stderr TEXT,
    result TEXT,
    result_media_type TEXT,
    error TEXT,
    error_media_type TEXT,
    config_snapshot TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS job_runs_unique_run ON job_runs(job_id, scheduled_for);

CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL,
    tenant_id TEXT,
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

-- Secrets subsystem (see SECRETS.md). Only ciphertext, sealed key blobs, and public
-- keys live here. The KEK plaintext never touches the DB.
CREATE TABLE IF NOT EXISTS secrets (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    value_ct BLOB NOT NULL,
    value_nonce BLOB NOT NULL,
    aead_algo TEXT NOT NULL,
    dek_wrapped BLOB NOT NULL,
    kek_version INTEGER NOT NULL,
    tenant_id BLOB NOT NULL DEFAULT x'00000000000000000000000000000001',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (tenant_id, name)
);

CREATE TABLE IF NOT EXISTS kek_versions (
    version INTEGER PRIMARY KEY,
    state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    retired_at TEXT
);

CREATE TABLE IF NOT EXISTS node_keys (
    node_id TEXT NOT NULL,
    key_version INTEGER NOT NULL,
    public_key BLOB NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    approved_at TEXT,
    PRIMARY KEY (node_id, key_version)
);

CREATE TABLE IF NOT EXISTS kek_shares (
    version INTEGER NOT NULL,
    node_id TEXT NOT NULL,
    wrapped_kek BLOB NOT NULL,
    acked_at TEXT,
    PRIMARY KEY (version, node_id)
);

-- Append-only run output, one chunk per flush per stream (source of truth for output).
CREATE TABLE IF NOT EXISTS run_log_chunks (
    run_id TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    stream TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (run_id, attempt, seq)
);

-- Shared DB connection configs (per engine). password_secret holds a secret reference
-- (secret:<name>), never a plaintext password. Storing configs is backend-agnostic;
-- executing pgsql/mysql runners is still Postgres-only.
CREATE TABLE IF NOT EXISTS pgsql_configs (
    id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 5432,
    username TEXT NOT NULL,
    password_secret TEXT NOT NULL,
    database TEXT NOT NULL,
    tenant_id BLOB NOT NULL DEFAULT x'00000000000000000000000000000001',
    deleted_at TEXT DEFAULT NULL,
    UNIQUE (host, port, username, database)
);

CREATE TABLE IF NOT EXISTS mysql_configs (
    id BLOB PRIMARY KEY,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL DEFAULT 3306,
    username TEXT NOT NULL,
    password_secret TEXT NOT NULL,
    database TEXT NOT NULL,
    tenant_id BLOB NOT NULL DEFAULT x'00000000000000000000000000000001',
    deleted_at TEXT DEFAULT NULL,
    UNIQUE (host, port, username, database)
);
