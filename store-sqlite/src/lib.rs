//! Embedded single-node SQLite backend implementing the arbiter `Store` traits.
//!
//! Uses sqlx's compile-time-checked `query!` macros. Because SQLite is dynamically
//! typed, output columns carry explicit overrides (`id!: Uuid`, timestamps as
//! `DateTime<Utc>`). Building offline uses the committed `.sqlx` cache; regenerate
//! it with `cargo sqlx prepare` against a SQLite DB built from `schema.sql`.
//!
//! Claims are a single `UPDATE ... RETURNING` under WAL + `busy_timeout`, which
//! serializes writers correctly without SQLite's read-then-write deadlock.
//! Leadership uses a lease row; the reaper keys off `last_seen`.

use std::str::FromStr;

use std::collections::HashMap;

use arbiter_core::{
    ApiStore, ArbiterError, ExecutableConfigSnapshot, ExecutableConfigSnapshotMeta, JobRun,
    JobRunState, JobSpec, JobStore, MisfirePolicy, Result, RunStore, RunnerConfig, Setting,
    SettingsStore, Store, User, UserRole, WorkerRecord, WorkerStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use uuid::Uuid;

const SCHEMA: &str = include_str!("../schema.sql");

fn db<E: std::fmt::Display>(e: E) -> ArbiterError {
    ArbiterError::DatabaseError(e.to_string())
}

#[allow(clippy::too_many_arguments)]
fn mk_run(
    id: Uuid,
    job_id: Uuid,
    scheduled_for: DateTime<Utc>,
    state: String,
    worker_id: Option<Uuid>,
    exit_code: Option<i64>,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    output: Option<String>,
    error_output: Option<String>,
) -> Result<JobRun> {
    Ok(JobRun {
        id,
        job_id,
        scheduled_for,
        state: JobRunState::from_str(&state)?,
        worker_id,
        exit_code: exit_code.map(|v| v as i32),
        started_at,
        finished_at,
        snapshot: None,
        output,
        error_output,
    })
}

#[allow(clippy::too_many_arguments)]
fn mk_job_spec(
    id: Uuid,
    name: String,
    schedule_cron: Option<String>,
    enabled: bool,
    runner_type: String,
    max_concurrency: i64,
    misfire_policy: String,
    command: Option<String>,
    working_dir: Option<String>,
    http_method: Option<String>,
    http_url: Option<String>,
    http_headers: Option<String>,
    http_body: Option<String>,
    http_timeout_sec: Option<i64>,
    py_module: Option<String>,
    py_class_name: Option<String>,
    py_timeout_sec: Option<i64>,
    node_module: Option<String>,
    node_function_name: Option<String>,
    node_timeout_sec: Option<i64>,
) -> Result<JobSpec> {
    let runner_cfg = match runner_type.as_str() {
        "shell" => RunnerConfig::Shell {
            command: command.unwrap_or_default(),
            working_dir,
        },
        "http" => RunnerConfig::Http {
            method: http_method.unwrap_or_else(|| "GET".to_string()),
            url: http_url.unwrap_or_default(),
            headers: http_headers.and_then(|j| serde_json::from_str(&j).ok()),
            body: http_body,
            timeout_sec: http_timeout_sec.map(|x| x as u32),
        },
        "python" => RunnerConfig::Python {
            module: py_module.unwrap_or_default(),
            class_name: py_class_name.unwrap_or_default(),
            timeout_sec: py_timeout_sec.map(|x| x as u32),
        },
        "node" => RunnerConfig::Node {
            module: node_module.unwrap_or_default(),
            function_name: node_function_name.unwrap_or_default(),
            timeout_sec: node_timeout_sec.map(|x| x as u32),
        },
        // pgsql/mysql need shared connection configs + secrets, not yet on sqlite.
        other => {
            return Err(ArbiterError::ExecutionError(format!(
                "runner '{other}' not supported in the sqlite backend yet"
            )));
        }
    };
    Ok(JobSpec {
        id,
        name,
        schedule_cron,
        enabled,
        runner_cfg,
        max_concurrency: max_concurrency as u32,
        misfire_policy: MisfirePolicy::from_str(&misfire_policy)?,
    })
}

fn mk_user(
    id: Uuid,
    username: String,
    password_hash: String,
    role: String,
    created_at: DateTime<Utc>,
) -> Result<User> {
    Ok(User {
        id,
        username,
        password_hash,
        role: UserRole::from_str(&role)?,
        created_at,
    })
}

pub struct SqliteStore {
    pool: SqlitePool,
    node_id: Uuid,
}

impl SqliteStore {
    /// Open (creating if needed) a SQLite store at `path` and apply the schema.
    pub async fn connect(path: &str) -> Result<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .map_err(db)?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await.map_err(db)?;
        Ok(Self {
            pool,
            node_id: Uuid::new_v4(),
        })
    }

    /// Load a job's environment variables, injected into subprocess runners.
    async fn load_env_for_job(&self, job_id: Uuid) -> Result<HashMap<String, String>> {
        let rows = sqlx::query!(
            r#"SELECT key AS "key!", value AS "value!" FROM job_env_vars WHERE job_id = ?"#,
            job_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    /// Build the executable snapshot for a job from its current runner config, so a
    /// claimed run carries everything the worker needs (independent of later edits).
    async fn build_snapshot_for_job(&self, job_id: Uuid) -> Result<ExecutableConfigSnapshot> {
        let row = sqlx::query!(
            r#"SELECT j.name AS "name!", j.runner_type AS "runner_type!",
                      s.command AS "shell_command?", s.working_dir AS "shell_working_dir?",
                      h.method AS "http_method?", h.url AS "http_url?",
                      h.headers AS "http_headers?", h.body AS "http_body?",
                      h.timeout_sec AS "http_timeout_sec?: i64",
                      py.module AS "py_module?", py.class_name AS "py_class_name?",
                      py.timeout_sec AS "py_timeout_sec?: i64",
                      nd.module AS "node_module?", nd.function_name AS "node_function_name?",
                      nd.timeout_sec AS "node_timeout_sec?: i64"
               FROM jobs j
               LEFT JOIN job_runner_shell s ON s.job_id = j.id
               LEFT JOIN job_runner_http h ON h.job_id = j.id
               LEFT JOIN job_runner_python py ON py.job_id = j.id
               LEFT JOIN job_runner_node nd ON nd.job_id = j.id
               WHERE j.id = ?"#,
            job_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        let row = row.ok_or_else(|| ArbiterError::NotFound(format!("job {job_id}")))?;

        let meta = match row.runner_type.as_str() {
            "shell" => ExecutableConfigSnapshotMeta::Shell {
                command: row.shell_command.unwrap_or_default(),
                working_dir: row.shell_working_dir,
                env: self.load_env_for_job(job_id).await?,
            },
            "http" => {
                let headers = row
                    .http_headers
                    .and_then(|j| serde_json::from_str::<HashMap<String, String>>(&j).ok())
                    .unwrap_or_default();
                ExecutableConfigSnapshotMeta::Http {
                    method: row.http_method.unwrap_or_else(|| "GET".to_string()),
                    url: row.http_url.unwrap_or_default(),
                    headers,
                    body: row.http_body,
                    timeout_sec: row.http_timeout_sec.map(|x| x as u32),
                }
            }
            "python" => ExecutableConfigSnapshotMeta::Python {
                module: row.py_module.unwrap_or_default(),
                class_name: row.py_class_name.unwrap_or_default(),
                timeout_sec: row.py_timeout_sec.map(|x| x as u32),
                env: self.load_env_for_job(job_id).await?,
            },
            "node" => ExecutableConfigSnapshotMeta::Node {
                module: row.node_module.unwrap_or_default(),
                function_name: row.node_function_name.unwrap_or_default(),
                timeout_sec: row.node_timeout_sec.map(|x| x as u32),
                env: self.load_env_for_job(job_id).await?,
            },
            other => {
                return Err(ArbiterError::ExecutionError(format!(
                    "runner '{other}' not supported in the sqlite backend yet"
                )));
            }
        };

        Ok(ExecutableConfigSnapshot {
            name: None,
            job_name: row.name,
            meta,
        })
    }
}

impl Store for SqliteStore {}

#[async_trait]
impl JobStore for SqliteStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>> {
        let rows = sqlx::query!(
            r#"SELECT j.id AS "id!: Uuid", j.name AS "name!", j.schedule_cron,
                      j.enabled AS "enabled!: bool", j.runner_type AS "runner_type!",
                      j.max_concurrency AS "max_concurrency!: i64",
                      j.misfire_policy AS "misfire_policy!", s.command AS "command?", s.working_dir,
                      h.method AS "http_method?", h.url AS "http_url?",
                      h.headers AS "http_headers?", h.body AS "http_body?",
                      h.timeout_sec AS "http_timeout_sec?: i64",
                      py.module AS "py_module?", py.class_name AS "py_class_name?",
                      py.timeout_sec AS "py_timeout_sec?: i64",
                      nd.module AS "node_module?", nd.function_name AS "node_function_name?",
                      nd.timeout_sec AS "node_timeout_sec?: i64"
               FROM jobs j LEFT JOIN job_runner_shell s ON s.job_id = j.id
               LEFT JOIN job_runner_http h ON h.job_id = j.id
               LEFT JOIN job_runner_python py ON py.job_id = j.id
               LEFT JOIN job_runner_node nd ON nd.job_id = j.id
               WHERE j.deleted_at IS NULL AND j.enabled = 1 AND j.schedule_cron IS NOT NULL"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                mk_job_spec(
                    r.id,
                    r.name,
                    r.schedule_cron,
                    r.enabled,
                    r.runner_type,
                    r.max_concurrency,
                    r.misfire_policy,
                    r.command,
                    r.working_dir,
                    r.http_method,
                    r.http_url,
                    r.http_headers,
                    r.http_body,
                    r.http_timeout_sec,
                    r.py_module,
                    r.py_class_name,
                    r.py_timeout_sec,
                    r.node_module,
                    r.node_function_name,
                    r.node_timeout_sec,
                )
            })
            .collect()
    }

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool> {
        let id = Uuid::new_v4();
        let res = sqlx::query!(
            "INSERT OR IGNORE INTO job_runs (id, job_id, scheduled_for, state) VALUES (?, ?, ?, 'queued')",
            id,
            job_id,
            scheduled_for
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(res.rows_affected() == 1)
    }
}

#[async_trait]
impl RunStore for SqliteStore {
    async fn prune_runs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let res = sqlx::query!(
            "DELETE FROM job_runs WHERE scheduled_for < ? AND state IN ('succeeded', 'failed', 'cancelled')",
            older_than
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(res.rows_affected())
    }

    async fn claim_job_runs(&self, worker_id: Uuid, limit: u32) -> Result<Vec<JobRun>> {
        let now = Utc::now();
        let limit = limit as i64;
        let rows = sqlx::query!(
            r#"UPDATE job_runs SET state = 'running', worker_id = ?, started_at = ?
               WHERE id IN (
                   SELECT jr.id FROM job_runs jr JOIN jobs j ON j.id = jr.job_id
                   WHERE jr.state = 'queued' AND jr.scheduled_for <= ?
                     AND j.enabled = 1 AND j.deleted_at IS NULL
                   ORDER BY jr.scheduled_for LIMIT ?
               )
               RETURNING id AS "id!: Uuid", job_id AS "job_id!: Uuid",
                         scheduled_for AS "scheduled_for!: DateTime<Utc>", state AS "state!",
                         worker_id AS "worker_id?: Uuid", exit_code,
                         started_at AS "started_at?: DateTime<Utc>",
                         finished_at AS "finished_at?: DateTime<Utc>", output, error_output"#,
            worker_id,
            now,
            now,
            limit
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        // Build, persist, and attach the config snapshot for each claimed run so the
        // worker has everything it needs (PG does this inside claim; do the same here).
        let mut runs = Vec::with_capacity(rows.len());
        for r in rows {
            let snapshot = self.build_snapshot_for_job(r.job_id).await?;
            let snapshot_json = serde_json::to_string(&snapshot)
                .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;
            sqlx::query!(
                "UPDATE job_runs SET config_snapshot = ? WHERE id = ?",
                snapshot_json,
                r.id
            )
            .execute(&self.pool)
            .await
            .map_err(db)?;

            let mut run = mk_run(
                r.id,
                r.job_id,
                r.scheduled_for,
                r.state,
                r.worker_id,
                r.exit_code,
                r.started_at,
                r.finished_at,
                r.output,
                r.error_output,
            )?;
            run.snapshot = Some(snapshot);
            runs.push(run);
        }
        Ok(runs)
    }

    async fn update_job_run_state(
        &self,
        run_id: Uuid,
        new_state: JobRunState,
        exit_code: Option<i32>,
        output: Option<String>,
        error_output: Option<String>,
    ) -> Result<()> {
        let finished_at = matches!(
            new_state,
            JobRunState::Succeeded | JobRunState::Failed | JobRunState::Cancelled
        )
        .then_some(Utc::now());
        let state = new_state.to_string();
        let exit_code = exit_code.map(|c| c as i64);
        sqlx::query!(
            "UPDATE job_runs SET state = ?, exit_code = ?, output = ?, error_output = ?, finished_at = ? WHERE id = ?",
            state,
            exit_code,
            output,
            error_output,
            finished_at,
            run_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}

#[async_trait]
impl WorkerStore for SqliteStore {
    async fn heartbeat(&self, worker: &WorkerRecord) -> Result<()> {
        let capacity = worker.capacity as i64;
        let restart_count = worker.restart_count as i64;
        sqlx::query!(
            "INSERT INTO workers (id, display_name, hostname, last_seen, capacity, active, restart_count, version) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET last_seen = excluded.last_seen, hostname = excluded.hostname, \
                capacity = excluded.capacity, version = excluded.version, active = 1",
            worker.id,
            worker.display_name,
            worker.hostname,
            worker.last_seen,
            capacity,
            restart_count,
            worker.version
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn lookup_by_id(&self, id: Uuid) -> Result<Option<(String, u32)>> {
        let row = sqlx::query!(
            r#"SELECT display_name AS "display_name!", capacity AS "capacity!: i64" FROM workers WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| (r.display_name, r.capacity as u32)))
    }

    async fn incr_restart_count(&self, id: Uuid, version: &str) -> Result<u32> {
        let row = sqlx::query!(
            r#"UPDATE workers SET restart_count = restart_count + 1, version = ? WHERE id = ?
               RETURNING restart_count AS "restart_count!: i64""#,
            version,
            id
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.restart_count as u32)
    }

    async fn insert_worker(
        &self,
        id: Uuid,
        display_name: &str,
        hostname: &str,
        version: &str,
        restart_count: u32,
    ) -> Result<()> {
        let now = Utc::now();
        let capacity = 4_i64;
        let restart_count = restart_count as i64;
        sqlx::query!(
            "INSERT INTO workers (id, display_name, hostname, last_seen, capacity, active, restart_count, version) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
            id,
            display_name,
            hostname,
            now,
            capacity,
            restart_count,
            version
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn reclaim_dead_workers_jobs(&self, dead_after_secs: u32) -> Result<u64> {
        let threshold = Utc::now() - Duration::seconds(dead_after_secs as i64);
        let res = sqlx::query!(
            "UPDATE job_runs SET state = 'queued', worker_id = NULL, started_at = NULL \
             WHERE state = 'running' AND worker_id IN (SELECT id FROM workers WHERE last_seen < ?)",
            threshold
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(res.rows_affected())
    }

    async fn am_i_leader(&self) -> Result<bool> {
        let now = Utc::now();
        let expires = now + Duration::seconds(10);
        let res = sqlx::query!(
            "INSERT INTO leader_lease (id, holder, expires_at) VALUES (1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at \
             WHERE leader_lease.holder = ? OR leader_lease.expires_at <= ?",
            self.node_id,
            expires,
            self.node_id,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        if res.rows_affected() > 0 {
            return Ok(true);
        }
        let holder = sqlx::query_scalar!(
            r#"SELECT holder AS "holder?: Uuid" FROM leader_lease WHERE id = 1"#
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?
        .flatten();
        Ok(holder == Some(self.node_id))
    }
}

#[async_trait]
impl ApiStore for SqliteStore {
    async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn get_job(&self, job_id: Uuid) -> Result<JobSpec> {
        let row = sqlx::query!(
            r#"SELECT j.id AS "id!: Uuid", j.name AS "name!", j.schedule_cron,
                      j.enabled AS "enabled!: bool", j.runner_type AS "runner_type!",
                      j.max_concurrency AS "max_concurrency!: i64",
                      j.misfire_policy AS "misfire_policy!", s.command AS "command?", s.working_dir,
                      h.method AS "http_method?", h.url AS "http_url?",
                      h.headers AS "http_headers?", h.body AS "http_body?",
                      h.timeout_sec AS "http_timeout_sec?: i64",
                      py.module AS "py_module?", py.class_name AS "py_class_name?",
                      py.timeout_sec AS "py_timeout_sec?: i64",
                      nd.module AS "node_module?", nd.function_name AS "node_function_name?",
                      nd.timeout_sec AS "node_timeout_sec?: i64"
               FROM jobs j LEFT JOIN job_runner_shell s ON s.job_id = j.id
               LEFT JOIN job_runner_http h ON h.job_id = j.id
               LEFT JOIN job_runner_python py ON py.job_id = j.id
               LEFT JOIN job_runner_node nd ON nd.job_id = j.id
               WHERE j.deleted_at IS NULL AND j.id = ?"#,
            job_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(r) => mk_job_spec(
                r.id,
                r.name,
                r.schedule_cron,
                r.enabled,
                r.runner_type,
                r.max_concurrency,
                r.misfire_policy,
                r.command,
                r.working_dir,
                r.http_method,
                r.http_url,
                r.http_headers,
                r.http_body,
                r.http_timeout_sec,
                r.py_module,
                r.py_class_name,
                r.py_timeout_sec,
                r.node_module,
                r.node_function_name,
                r.node_timeout_sec,
            ),
            None => Err(ArbiterError::NotFound(format!("job {job_id}"))),
        }
    }

    async fn create_job(
        &self,
        name: &str,
        schedule_cron: Option<String>,
        runner_cfg: RunnerConfig,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
    ) -> Result<JobSpec> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let runner_type = runner_cfg.type_of_str();
        let mc = max_concurrency as i64;
        let mp = misfire_policy.to_string();
        sqlx::query!(
            "INSERT INTO jobs (id, name, schedule_cron, enabled, runner_type, max_concurrency, created_at, misfire_policy) \
             VALUES (?, ?, ?, 0, ?, ?, ?, ?)",
            id,
            name,
            schedule_cron,
            runner_type,
            mc,
            now,
            mp
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;

        match &runner_cfg {
            RunnerConfig::Shell {
                command,
                working_dir,
            } => {
                sqlx::query!(
                    "INSERT INTO job_runner_shell (job_id, command, working_dir) VALUES (?, ?, ?)",
                    id,
                    command,
                    working_dir
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            RunnerConfig::Http {
                method,
                url,
                headers,
                body,
                timeout_sec,
            } => {
                let headers_json = headers
                    .as_ref()
                    .map(|h| serde_json::to_string(h).unwrap_or_default());
                let timeout = timeout_sec.as_ref().map(|t| *t as i64);
                sqlx::query!(
                    "INSERT INTO job_runner_http (job_id, method, url, headers, body, timeout_sec) VALUES (?, ?, ?, ?, ?, ?)",
                    id,
                    method,
                    url,
                    headers_json,
                    body,
                    timeout
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            RunnerConfig::Python {
                module,
                class_name,
                timeout_sec,
            } => {
                let timeout = timeout_sec.as_ref().map(|t| *t as i64);
                sqlx::query!(
                    "INSERT INTO job_runner_python (job_id, module, class_name, timeout_sec) VALUES (?, ?, ?, ?)",
                    id,
                    module,
                    class_name,
                    timeout
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            RunnerConfig::Node {
                module,
                function_name,
                timeout_sec,
            } => {
                let timeout = timeout_sec.as_ref().map(|t| *t as i64);
                sqlx::query!(
                    "INSERT INTO job_runner_node (job_id, module, function_name, timeout_sec) VALUES (?, ?, ?, ?)",
                    id,
                    module,
                    function_name,
                    timeout
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            // pgsql/mysql need shared connection configs + secrets, not yet on sqlite.
            other => {
                return Err(ArbiterError::ExecutionError(format!(
                    "runner '{}' not supported in the sqlite backend yet",
                    other.type_of_str()
                )));
            }
        }

        Ok(JobSpec {
            id,
            name: name.to_string(),
            schedule_cron,
            enabled: false,
            runner_cfg,
            max_concurrency,
            misfire_policy,
        })
    }

    async fn list_jobs(&self) -> Result<Vec<JobSpec>> {
        let rows = sqlx::query!(
            r#"SELECT j.id AS "id!: Uuid", j.name AS "name!", j.schedule_cron,
                      j.enabled AS "enabled!: bool", j.runner_type AS "runner_type!",
                      j.max_concurrency AS "max_concurrency!: i64",
                      j.misfire_policy AS "misfire_policy!", s.command AS "command?", s.working_dir,
                      h.method AS "http_method?", h.url AS "http_url?",
                      h.headers AS "http_headers?", h.body AS "http_body?",
                      h.timeout_sec AS "http_timeout_sec?: i64",
                      py.module AS "py_module?", py.class_name AS "py_class_name?",
                      py.timeout_sec AS "py_timeout_sec?: i64",
                      nd.module AS "node_module?", nd.function_name AS "node_function_name?",
                      nd.timeout_sec AS "node_timeout_sec?: i64"
               FROM jobs j LEFT JOIN job_runner_shell s ON s.job_id = j.id
               LEFT JOIN job_runner_http h ON h.job_id = j.id
               LEFT JOIN job_runner_python py ON py.job_id = j.id
               LEFT JOIN job_runner_node nd ON nd.job_id = j.id
               WHERE j.deleted_at IS NULL"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                mk_job_spec(
                    r.id,
                    r.name,
                    r.schedule_cron,
                    r.enabled,
                    r.runner_type,
                    r.max_concurrency,
                    r.misfire_policy,
                    r.command,
                    r.working_dir,
                    r.http_method,
                    r.http_url,
                    r.http_headers,
                    r.http_body,
                    r.http_timeout_sec,
                    r.py_module,
                    r.py_class_name,
                    r.py_timeout_sec,
                    r.node_module,
                    r.node_function_name,
                    r.node_timeout_sec,
                )
            })
            .collect()
    }

    async fn list_recent_runs(
        &self,
        limit: Option<u32>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
        by_worker_id: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        let limit = limit.map(|l| l as i64);
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", job_id AS "job_id!: Uuid",
                      scheduled_for AS "scheduled_for!: DateTime<Utc>", state AS "state!",
                      worker_id AS "worker_id?: Uuid", exit_code,
                      started_at AS "started_at?: DateTime<Utc>",
                      finished_at AS "finished_at?: DateTime<Utc>", output, error_output
               FROM job_runs
               WHERE (?1 IS NULL OR job_id = ?1)
                 AND (?2 IS NULL OR worker_id = ?2)
                 AND (?3 IS NULL OR scheduled_for < ?3)
                 AND (?4 IS NULL OR scheduled_for > ?4)
               ORDER BY scheduled_for DESC
               LIMIT (CASE WHEN ?5 IS NULL THEN -1 ELSE ?5 END)"#,
            by_job_id,
            by_worker_id,
            before,
            after,
            limit
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                mk_run(
                    r.id,
                    r.job_id,
                    r.scheduled_for,
                    r.state,
                    r.worker_id,
                    r.exit_code,
                    r.started_at,
                    r.finished_at,
                    r.output,
                    r.error_output,
                )
            })
            .collect()
    }

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()> {
        sqlx::query!("UPDATE jobs SET enabled = ? WHERE id = ?", enabled, job_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn enable_job(&self, job_id: Uuid) -> Result<()> {
        self.set_job_enabled(job_id, true).await
    }

    async fn disable_job(&self, job_id: Uuid) -> Result<()> {
        self.set_job_enabled(job_id, false).await
    }

    async fn update_job(
        &self,
        job_id: Uuid,
        name: Option<String>,
        schedule_cron: Option<Option<String>>,
        runner_cfg: Option<RunnerConfig>,
        max_concurrency: Option<u32>,
        misfire_policy: Option<MisfirePolicy>,
    ) -> Result<JobSpec> {
        // Outer Some = update schedule_cron (to the inner value, which may be NULL);
        // None = leave it unchanged. COALESCE handles the other optional fields.
        let cron_set = schedule_cron.is_some();
        let cron_val = schedule_cron.flatten();
        let max_concurrency = max_concurrency.map(|v| v as i64);
        let misfire_policy = misfire_policy.map(|p| p.to_string());

        sqlx::query!(
            "UPDATE jobs SET
                name = COALESCE(?, name),
                schedule_cron = CASE WHEN ? THEN ? ELSE schedule_cron END,
                max_concurrency = COALESCE(?, max_concurrency),
                misfire_policy = COALESCE(?, misfire_policy)
             WHERE id = ? AND deleted_at IS NULL",
            name,
            cron_set,
            cron_val,
            max_concurrency,
            misfire_policy,
            job_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;

        if let Some(cfg) = runner_cfg {
            match cfg {
                RunnerConfig::Shell {
                    command,
                    working_dir,
                } => {
                    sqlx::query!("UPDATE jobs SET runner_type = 'shell' WHERE id = ?", job_id)
                        .execute(&self.pool)
                        .await
                        .map_err(db)?;
                    sqlx::query!(
                        "INSERT INTO job_runner_shell (job_id, command, working_dir) VALUES (?, ?, ?) \
                         ON CONFLICT(job_id) DO UPDATE SET command = excluded.command, working_dir = excluded.working_dir",
                        job_id,
                        command,
                        working_dir
                    )
                    .execute(&self.pool)
                    .await
                    .map_err(db)?;
                }
                other => {
                    return Err(ArbiterError::ExecutionError(format!(
                        "runner '{}' not supported in the sqlite backend yet",
                        other.type_of_str()
                    )));
                }
            }
        }

        self.get_job(job_id).await
    }

    async fn delete_job(&self, job_id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query!("UPDATE jobs SET deleted_at = ? WHERE id = ?", now, job_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query!(
            "INSERT INTO job_runs (id, job_id, scheduled_for, state) VALUES (?, ?, ?, 'queued')",
            id,
            job_id,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(JobRun {
            id,
            job_id,
            scheduled_for: now,
            state: JobRunState::Queued,
            worker_id: None,
            exit_code: None,
            started_at: None,
            finished_at: None,
            snapshot: None,
            output: None,
            error_output: None,
        })
    }

    async fn cancel_run(&self, run_id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query!(
            "UPDATE job_runs SET state = 'cancelled', finished_at = ? WHERE id = ?",
            now,
            run_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", display_name AS "display_name!", hostname AS "hostname!",
                      last_seen AS "last_seen!: DateTime<Utc>", capacity AS "capacity!: i64",
                      restart_count AS "restart_count!: i64", version AS "version!"
               FROM workers"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| WorkerRecord {
                id: r.id,
                display_name: r.display_name,
                hostname: r.hostname,
                last_seen: r.last_seen,
                capacity: r.capacity as u32,
                restart_count: r.restart_count as u32,
                version: r.version,
            })
            .collect())
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", username AS "username!", password_hash AS "password_hash!",
                      role AS "role!", created_at AS "created_at!: DateTime<Utc>"
               FROM users WHERE username = ?"#,
            username
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(r) => mk_user(r.id, r.username, r.password_hash, r.role, r.created_at),
            None => Err(ArbiterError::NotFound(format!("user {username}"))),
        }
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", username AS "username!", password_hash AS "password_hash!",
                      role AS "role!", created_at AS "created_at!: DateTime<Utc>"
               FROM users WHERE id = ?"#,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(r) => mk_user(r.id, r.username, r.password_hash, r.role, r.created_at),
            None => Err(ArbiterError::NotFound(format!("user {user_id}"))),
        }
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
    ) -> Result<User> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let role_s = role.to_string();
        sqlx::query!(
            "INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?)",
            id,
            username,
            password_hash,
            role_s,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(User {
            id,
            username: username.to_string(),
            password_hash: password_hash.to_string(),
            role,
            created_at: now,
        })
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", username AS "username!", password_hash AS "password_hash!",
                      role AS "role!", created_at AS "created_at!: DateTime<Utc>"
               FROM users"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| mk_user(r.id, r.username, r.password_hash, r.role, r.created_at))
            .collect()
    }

    async fn delete_user(&self, user_id: Uuid) -> Result<()> {
        sqlx::query!("DELETE FROM users WHERE id = ?", user_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<()> {
        sqlx::query!(
            "UPDATE users SET password_hash = ? WHERE id = ?",
            password_hash,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn update_user(
        &self,
        user_id: Uuid,
        username: Option<&str>,
        password_hash: Option<&str>,
        role: Option<UserRole>,
    ) -> Result<User> {
        let role = role.map(|r| r.to_string());
        sqlx::query!(
            "UPDATE users SET
                username = COALESCE(?, username),
                password_hash = COALESCE(?, password_hash),
                role = COALESCE(?, role)
             WHERE id = ?",
            username,
            password_hash,
            role,
            user_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        self.get_user_by_id(user_id).await
    }

    async fn count_users(&self) -> Result<u32> {
        let n = sqlx::query_scalar!(r#"SELECT COUNT(*) AS "n!: i64" FROM users"#)
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        Ok(n as u32)
    }
}

#[async_trait]
impl SettingsStore for SqliteStore {
    async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query!(r#"SELECT value AS "value!" FROM settings WHERE key = ?"#, key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        Ok(row.map(|r| r.value))
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let now = Utc::now();
        sqlx::query!(
            "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            key,
            value,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn list_settings(&self) -> Result<Vec<Setting>> {
        let rows = sqlx::query!(
            r#"SELECT key AS "key!", value AS "value!" FROM settings ORDER BY key"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| Setting {
                key: r.key,
                value: r.value,
            })
            .collect())
    }
}
