CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE workers (
    id UUID PRIMARY KEY,
    hostname TEXT NOT NULL,
    last_seen TIMESTAMPTZ NOT NULL,
    capacity INT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true
);

-- Name uniqueness? Or warning at least
CREATE TABLE jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name TEXT NOT NULL,
    schedule_cron TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    command TEXT NOT NULL,
    max_concurrency INT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    misfire_policy TEXT NOT NULL DEFAULT 'run_immediately',
    deleted_at TIMESTAMPTZ DEFAULT NULL
);

-- Add name for job_runs? In case Job is deleted
CREATE TABLE job_runs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    job_id UUID NOT NULL REFERENCES jobs(id),
    scheduled_for TIMESTAMPTZ NOT NULL,
    state TEXT NOT NULL,
    queue TEXT,
    worker_id UUID REFERENCES workers(id),
    queued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    exit_code INT,
    error_message TEXT,
    command TEXT NOT NULL
);

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
    WHERE enabled = TRUE;


