CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

----------------------------
-- Runner Types
----------------------------
CREATE TABLE runner_types (
    name TEXT PRIMARY KEY
);

INSERT INTO runner_types (name) VALUES
    ('shell'), ('http'), ('pgsql'), ('mysql'), ('python'), ('node');

----------------------------
-- Core Tables
----------------------------
CREATE TABLE workers (
    id UUID PRIMARY KEY,
    display_name TEXT NOT NULL,
    hostname TEXT NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL,
    capacity INT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true
);

-- Jobs are scheduler-level metadata + which runner they use

-- TODO: Separate scheduled and template jobs to run on demand with different values? retry techniques, acceptance config that it succeeded

-- TODO: Name uniqueness? Or warning at least
-- TODO: Secondary tables to match jobs with run configs? Might need field per config type though, but at least uniqueness can be enforced with constaint easily
-- TODO: Authentication step for HTTP runner. itself having optional retreival step(request + way to retrieve token. cookie? header? json?). Then how to use(header, cookie, etc)
-- TODO: SSH optional prepare step. To run commands on another host. SHould always run first. Like SSH, connect to db, run command query, etc. Or SSH, then run shell command
-- TODO: Add timeout(opt in?)
-- TODO: But how to handle rerun when config for job changed?
CREATE TABLE jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    schedule_cron TEXT,
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    runner_type TEXT NOT NULL REFERENCES runner_types(name),
    max_concurrency INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    misfire_policy TEXT NOT NULL DEFAULT 'run_immediately',
    -- TODO: Make use of for smaller indexes/efficiency on some queries?
    deleted_at TIMESTAMPTZ DEFAULT NULL
);

-- TODO: Rework constraint and/or policy storage
-- ALTER TABLE jobs
-- ADD CONSTRAINT misfire_policy_check CHECK (
--     misfire_policy IN ('run_immediately', 'skip', 'queue_once')
-- );

----------------------------
-- Environment Variables
----------------------------
CREATE TABLE job_env_vars (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    UNIQUE(job_id, key)
);

-- TODO: SSH configs

----------------------------
-- Shared DB Configs (per engine)
----------------------------

-- PostgreSQL connection configs (shared)
CREATE TABLE pgsql_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INT NOT NULL DEFAULT 5432,
    username TEXT NOT NULL,
    password_secret TEXT NOT NULL,     -- eventually a reference to secrets
    database TEXT NOT NULL,
    UNIQUE(host, port, username, database),
    deleted_at TIMESTAMPTZ DEFAULT NULL
);

-- MySQL connection configs (shared)
CREATE TABLE mysql_configs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INT NOT NULL DEFAULT 3306,
    username TEXT NOT NULL,
    password_secret TEXT NOT NULL,
    database TEXT NOT NULL,
    UNIQUE(host, port, username, database),
    deleted_at TIMESTAMPTZ DEFAULT NULL
);

----------------------------
-- Runner Config Tables
----------------------------

-- TODO: Unique constraints.

CREATE TABLE job_runner_shell (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    command TEXT NOT NULL,
    working_dir TEXT,
    shell_user TEXT
);

CREATE TABLE job_runner_http (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    method TEXT NOT NULL,
    url TEXT NOT NULL,
    headers JSONB,         -- TODO: maybe investigate normalized headers table
    body TEXT,
    timeout_sec INT
);

CREATE TABLE job_runner_pgsql (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    config_id UUID NOT NULL REFERENCES pgsql_configs(id) ON DELETE RESTRICT,
    query TEXT NOT NULL,
    timeout_sec INT
);

CREATE TABLE job_runner_mysql (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    config_id UUID NOT NULL REFERENCES mysql_configs(id) ON DELETE RESTRICT,
    query TEXT NOT NULL,
    timeout_sec INT
);

CREATE TABLE job_runner_python (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    module TEXT NOT NULL,
    class_name TEXT NOT NULL,
    timeout_sec INT
);

CREATE TABLE job_runner_node (
    job_id UUID PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    module TEXT NOT NULL,
    function_name TEXT NOT NULL,
    timeout_sec INT
);

----------------------------
-- Job Runs
----------------------------
-- TODO: Add a type to be compatible with future multiple runners?
-- TODO: Constraint to ensure config_snapshot is not NULL if not queued?
CREATE TABLE job_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    job_id UUID NOT NULL REFERENCES jobs(id),
    scheduled_for TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL,
    queue TEXT,                                             -- Optional logical queue, different lanes (based on capability? priority? Like for slower background work, gpu, io, or just high prio)
    worker_id UUID REFERENCES workers(id),
    queued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    exit_code INT,
    output JSONB,
    error_output TEXT,
    log TEXT,
    config_snapshot JSONB
);

-- Keep DB state in sync with Rust enum
ALTER TABLE job_runs
ADD CONSTRAINT job_run_state_check CHECK (
    state IN ('queued', 'running', 'succeeded', 'failed', 'cancelled')
    -- TODO: add 'lost' later if/when added in Rust
);

CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    username TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'tenant', 'operator', 'viewer')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

----------------------------
-- Indexes
----------------------------

-- Ensures we never create two job_runs for the same job at the same timestamp.
-- Supports INSERT ... ON CONFLICT(job_id, scheduled_for).
CREATE UNIQUE INDEX job_runs_unique_run
    ON job_runs(job_id, scheduled_for);


-- Optimizes the job-claiming query:
--   SELECT ... FROM job_runs
--   WHERE state='queued' AND scheduled_for <= now()
--   ORDER BY scheduled_for
--   FOR UPDATE SKIP LOCKED
-- This partial index is small and covers exactly the rows involved in queue polling.
CREATE INDEX job_runs_queued_sched_idx
    ON job_runs(state, scheduled_for)
    WHERE state = 'queued';


-- Speeds up listing recent runs:
--   SELECT ... FROM job_runs
--   ORDER BY scheduled_for DESC
--   LIMIT N
-- Also accelerates before/after cursor filters on scheduled_for.
CREATE INDEX job_runs_sched_desc_idx
    ON job_runs(scheduled_for DESC);


-- Optimizes dead-worker reclamation:
--   SELECT id FROM workers WHERE last_seen < now() - interval ...
-- Useful only if workers table is non-trivial in size.
CREATE INDEX workers_last_seen_idx
    ON workers(last_seen);


-- Filters enabled cron jobs quickly:
--   SELECT ... FROM jobs WHERE enabled = TRUE AND schedule_cron IS NOT NULL
-- Partial index keeps it small and avoids scanning disabled jobs.
CREATE INDEX jobs_enabled_idx
    ON jobs(enabled)
    WHERE enabled = TRUE AND schedule_cron IS NOT NULL;
