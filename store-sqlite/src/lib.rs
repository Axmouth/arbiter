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
    ApiStore, ArbiterError, BackoffStrategy, ConfigStore, DbEngine, ExecutableConfigSnapshot,
    ExecutableConfigSnapshotMeta, JobRun, JobRunState, JobSpec, JobStore, MisfirePolicy,
    ResultStatus, Result, RetryConfig, RunOutcome, RunStore, RunnerConfig, SecretMeta, SecretStore,
    Setting, SettingsStore, SharedDbConfig, Store, StoredKekShare, StoredKekVersion, StoredNodeKey,
    StoredSecret, Tenant, TenantStore, User, UserRole, WorkerRecord, WorkerStore,
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
    attempt: i64,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    result_status: Option<String>,
    stdout: Option<String>,
    stderr: Option<String>,
    result: Option<String>,
    result_media_type: Option<String>,
    error: Option<String>,
    error_media_type: Option<String>,
) -> Result<JobRun> {
    let result_status = match result_status {
        Some(s) => Some(ResultStatus::from_str(&s)?),
        None => None,
    };
    Ok(JobRun {
        id,
        job_id,
        scheduled_for,
        state: JobRunState::from_str(&state)?,
        worker_id,
        exit_code: exit_code.map(|v| v as i32),
        attempt: attempt as u32,
        started_at,
        finished_at,
        snapshot: None,
        result_status,
        stdout,
        stderr,
        result,
        result_media_type,
        error,
        error_media_type,
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
    max_attempts: i64,
    backoff_strategy: String,
    backoff_base_secs: i64,
    backoff_cap_secs: i64,
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
        max_attempts: max_attempts as u32,
        backoff_strategy: BackoffStrategy::from_str(&backoff_strategy)?,
        backoff_base_secs: backoff_base_secs as u32,
        backoff_cap_secs: backoff_cap_secs as u32,
    })
}

fn mk_user(
    id: Uuid,
    username: String,
    password_hash: String,
    role: String,
    tenant_id: Option<Uuid>,
    created_at: DateTime<Utc>,
) -> Result<User> {
    Ok(User {
        id,
        username,
        password_hash,
        role: UserRole::from_str(&role)?,
        tenant_id,
        created_at,
    })
}

pub struct SqliteStore {
    pool: SqlitePool,
    node_id: Uuid,
    /// In-process settings-change signal. Single-node, so a NOTIFY/LISTEN transport is
    /// unnecessary: writers and readers share this process.
    settings_notify: std::sync::Arc<tokio::sync::Notify>,
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
            settings_notify: std::sync::Arc::new(tokio::sync::Notify::new()),
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
                      j.misfire_policy AS "misfire_policy!",
                      j.max_attempts AS "max_attempts!: i64", j.backoff_strategy AS "backoff_strategy!",
                      j.backoff_base_secs AS "backoff_base_secs!: i64", j.backoff_cap_secs AS "backoff_cap_secs!: i64",
                      s.command AS "command?", s.working_dir,
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
                    r.max_attempts,
                    r.backoff_strategy,
                    r.backoff_base_secs,
                    r.backoff_cap_secs,
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

    async fn job_tenant(&self, job_id: Uuid) -> Result<Option<Uuid>> {
        let row = sqlx::query!(
            r#"SELECT tenant_id AS "tenant_id!: Uuid" FROM jobs WHERE id = ?"#,
            job_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| r.tenant_id))
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
                         worker_id AS "worker_id?: Uuid", exit_code, attempt AS "attempt!: i64",
                         started_at AS "started_at?: DateTime<Utc>",
                         finished_at AS "finished_at?: DateTime<Utc>""#,
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
                r.attempt,
                r.started_at,
                r.finished_at,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )?;
            run.snapshot = Some(snapshot);
            runs.push(run);
        }
        Ok(runs)
    }

    async fn finalize_run(
        &self,
        run_id: Uuid,
        new_state: JobRunState,
        outcome: RunOutcome,
    ) -> Result<()> {
        let RunOutcome {
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
        } = outcome;
        let finished_at = Utc::now();
        let state = new_state.to_string();
        let status = status.map(|s| s.to_string());
        let exit_code = exit_code.map(|c| c as i64);
        sqlx::query!(
            "UPDATE job_runs SET state = ?, result_status = ?, exit_code = ?, stdout = ?, \
             stderr = ?, result = ?, result_media_type = ?, error = ?, error_media_type = ?, \
             finished_at = ? WHERE id = ?",
            state,
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
            finished_at,
            run_id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn reschedule_for_retry(
        &self,
        run_id: Uuid,
        attempt: u32,
        scheduled_for: DateTime<Utc>,
        outcome: RunOutcome,
    ) -> Result<()> {
        let RunOutcome {
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
        } = outcome;
        let status = status.map(|s| s.to_string());
        let exit_code = exit_code.map(|c| c as i64);
        let attempt = attempt as i64;
        sqlx::query!(
            "UPDATE job_runs SET state = 'queued', worker_id = NULL, started_at = NULL, \
             finished_at = NULL, attempt = ?, scheduled_for = ?, result_status = ?, exit_code = ?, \
             stdout = ?, stderr = ?, result = ?, result_media_type = ?, error = ?, \
             error_media_type = ? WHERE id = ?",
            attempt,
            scheduled_for,
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
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

    async fn get_job(&self, job_id: Uuid, scope: Option<Uuid>) -> Result<JobSpec> {
        let row = sqlx::query!(
            r#"SELECT j.id AS "id!: Uuid", j.name AS "name!", j.schedule_cron,
                      j.enabled AS "enabled!: bool", j.runner_type AS "runner_type!",
                      j.max_concurrency AS "max_concurrency!: i64",
                      j.misfire_policy AS "misfire_policy!",
                      j.max_attempts AS "max_attempts!: i64", j.backoff_strategy AS "backoff_strategy!",
                      j.backoff_base_secs AS "backoff_base_secs!: i64", j.backoff_cap_secs AS "backoff_cap_secs!: i64",
                      s.command AS "command?", s.working_dir,
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
               WHERE j.deleted_at IS NULL AND j.id = ?1 AND (?2 IS NULL OR j.tenant_id = ?2)"#,
            job_id,
            scope
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
                r.max_attempts,
                r.backoff_strategy,
                r.backoff_base_secs,
                r.backoff_cap_secs,
            ),
            None => Err(ArbiterError::NotFound(format!("job {job_id}"))),
        }
    }

    async fn create_job(
        &self,
        tenant_id: Uuid,
        name: &str,
        schedule_cron: Option<String>,
        runner_cfg: RunnerConfig,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
        retry: RetryConfig,
    ) -> Result<JobSpec> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let runner_type = runner_cfg.type_of_str();
        let mc = max_concurrency as i64;
        let mp = misfire_policy.to_string();
        let ma = retry.max_attempts as i64;
        let bs = retry.backoff_strategy.to_string();
        let bb = retry.backoff_base_secs as i64;
        let bc = retry.backoff_cap_secs as i64;
        sqlx::query!(
            "INSERT INTO jobs (id, tenant_id, name, schedule_cron, enabled, runner_type, max_concurrency, created_at, misfire_policy, max_attempts, backoff_strategy, backoff_base_secs, backoff_cap_secs) \
             VALUES (?, ?, ?, ?, 0, ?, ?, ?, ?, ?, ?, ?, ?)",
            id,
            tenant_id,
            name,
            schedule_cron,
            runner_type,
            mc,
            now,
            mp,
            ma,
            bs,
            bb,
            bc
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
            max_attempts: retry.max_attempts,
            backoff_strategy: retry.backoff_strategy,
            backoff_base_secs: retry.backoff_base_secs,
            backoff_cap_secs: retry.backoff_cap_secs,
        })
    }

    async fn list_jobs(&self, scope: Option<Uuid>) -> Result<Vec<JobSpec>> {
        let rows = sqlx::query!(
            r#"SELECT j.id AS "id!: Uuid", j.name AS "name!", j.schedule_cron,
                      j.enabled AS "enabled!: bool", j.runner_type AS "runner_type!",
                      j.max_concurrency AS "max_concurrency!: i64",
                      j.misfire_policy AS "misfire_policy!",
                      j.max_attempts AS "max_attempts!: i64", j.backoff_strategy AS "backoff_strategy!",
                      j.backoff_base_secs AS "backoff_base_secs!: i64", j.backoff_cap_secs AS "backoff_cap_secs!: i64",
                      s.command AS "command?", s.working_dir,
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
               WHERE j.deleted_at IS NULL AND (?1 IS NULL OR j.tenant_id = ?1)"#,
            scope
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
                    r.max_attempts,
                    r.backoff_strategy,
                    r.backoff_base_secs,
                    r.backoff_cap_secs,
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
        scope: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        let limit = limit.map(|l| l as i64);
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", job_id AS "job_id!: Uuid",
                      scheduled_for AS "scheduled_for!: DateTime<Utc>", state AS "state!",
                      worker_id AS "worker_id?: Uuid", exit_code, attempt AS "attempt!: i64",
                      started_at AS "started_at?: DateTime<Utc>",
                      finished_at AS "finished_at?: DateTime<Utc>", result_status,
                      stdout, stderr, result, result_media_type, error, error_media_type
               FROM job_runs
               WHERE (?1 IS NULL OR job_id = ?1)
                 AND (?2 IS NULL OR worker_id = ?2)
                 AND (?3 IS NULL OR scheduled_for < ?3)
                 AND (?4 IS NULL OR scheduled_for > ?4)
                 AND (?6 IS NULL OR job_id IN (SELECT id FROM jobs WHERE tenant_id = ?6))
               ORDER BY scheduled_for DESC
               LIMIT (CASE WHEN ?5 IS NULL THEN -1 ELSE ?5 END)"#,
            by_job_id,
            by_worker_id,
            before,
            after,
            limit,
            scope
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
                    r.attempt,
                    r.started_at,
                    r.finished_at,
                    r.result_status,
                    r.stdout,
                    r.stderr,
                    r.result,
                    r.result_media_type,
                    r.error,
                    r.error_media_type,
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
        retry: Option<RetryConfig>,
    ) -> Result<JobSpec> {
        // Outer Some = update schedule_cron (to the inner value, which may be NULL);
        // None = leave it unchanged. COALESCE handles the other optional fields.
        let cron_set = schedule_cron.is_some();
        let cron_val = schedule_cron.flatten();
        let max_concurrency = max_concurrency.map(|v| v as i64);
        let misfire_policy = misfire_policy.map(|p| p.to_string());
        let max_attempts = retry.map(|r| r.max_attempts as i64);
        let backoff_strategy = retry.map(|r| r.backoff_strategy.to_string());
        let backoff_base_secs = retry.map(|r| r.backoff_base_secs as i64);
        let backoff_cap_secs = retry.map(|r| r.backoff_cap_secs as i64);

        sqlx::query!(
            "UPDATE jobs SET
                name = COALESCE(?, name),
                schedule_cron = CASE WHEN ? THEN ? ELSE schedule_cron END,
                max_concurrency = COALESCE(?, max_concurrency),
                misfire_policy = COALESCE(?, misfire_policy),
                max_attempts = COALESCE(?, max_attempts),
                backoff_strategy = COALESCE(?, backoff_strategy),
                backoff_base_secs = COALESCE(?, backoff_base_secs),
                backoff_cap_secs = COALESCE(?, backoff_cap_secs)
             WHERE id = ? AND deleted_at IS NULL",
            name,
            cron_set,
            cron_val,
            max_concurrency,
            misfire_policy,
            max_attempts,
            backoff_strategy,
            backoff_base_secs,
            backoff_cap_secs,
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

        self.get_job(job_id, None).await
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
            attempt: 1,
            started_at: None,
            finished_at: None,
            snapshot: None,
            result_status: None,
            stdout: None,
            stderr: None,
            result: None,
            result_media_type: None,
            error: None,
            error_media_type: None,
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

    async fn set_job_env(&self, job_id: Uuid, env: HashMap<String, String>) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query!("DELETE FROM job_env_vars WHERE job_id = ?", job_id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for (key, value) in &env {
            sqlx::query!(
                "INSERT INTO job_env_vars (job_id, key, value) VALUES (?, ?, ?)",
                job_id,
                key,
                value
            )
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        Ok(())
    }

    async fn get_job_env(&self, job_id: Uuid) -> Result<HashMap<String, String>> {
        let rows = sqlx::query!(
            r#"SELECT key AS "key!", value AS "value!" FROM job_env_vars WHERE job_id = ?"#,
            job_id
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
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
                      role AS "role!", tenant_id AS "tenant_id?: Uuid", created_at AS "created_at!: DateTime<Utc>"
               FROM users WHERE username = ?"#,
            username
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(r) => mk_user(r.id, r.username, r.password_hash, r.role, r.tenant_id, r.created_at),
            None => Err(ArbiterError::NotFound(format!("user {username}"))),
        }
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", username AS "username!", password_hash AS "password_hash!",
                      role AS "role!", tenant_id AS "tenant_id?: Uuid", created_at AS "created_at!: DateTime<Utc>"
               FROM users WHERE id = ?"#,
            user_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        match row {
            Some(r) => mk_user(r.id, r.username, r.password_hash, r.role, r.tenant_id, r.created_at),
            None => Err(ArbiterError::NotFound(format!("user {user_id}"))),
        }
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
        tenant_id: Option<Uuid>,
    ) -> Result<User> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let role_s = role.to_string();
        sqlx::query!(
            "INSERT INTO users (id, username, password_hash, role, tenant_id, created_at) VALUES (?, ?, ?, ?, ?, ?)",
            id,
            username,
            password_hash,
            role_s,
            tenant_id,
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
            tenant_id,
            created_at: now,
        })
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", username AS "username!", password_hash AS "password_hash!",
                      role AS "role!", tenant_id AS "tenant_id?: Uuid", created_at AS "created_at!: DateTime<Utc>"
               FROM users"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| mk_user(r.id, r.username, r.password_hash, r.role, r.tenant_id, r.created_at))
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
        self.settings_notify.notify_waiters();
        Ok(())
    }

    async fn await_settings_change(&self) {
        self.settings_notify.notified().await;
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

#[async_trait]
impl SecretStore for SqliteStore {
    async fn upsert_secret(
        &self,
        tenant_id: Uuid,
        name: &str,
        value_ct: &[u8],
        value_nonce: &[u8],
        aead_algo: &str,
        dek_wrapped: &[u8],
        kek_version: u32,
    ) -> Result<Uuid> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let ver = kek_version as i64;
        let row = sqlx::query!(
            r#"INSERT INTO secrets (id, tenant_id, name, value_ct, value_nonce, aead_algo, dek_wrapped, kek_version, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(tenant_id, name) DO UPDATE SET
                   value_ct = excluded.value_ct, value_nonce = excluded.value_nonce,
                   aead_algo = excluded.aead_algo, dek_wrapped = excluded.dek_wrapped,
                   kek_version = excluded.kek_version, updated_at = excluded.updated_at
               RETURNING id AS "id!: Uuid""#,
            id,
            tenant_id,
            name,
            value_ct,
            value_nonce,
            aead_algo,
            dek_wrapped,
            ver,
            now,
            now
        )
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.id)
    }

    async fn get_secret_by_name(&self, tenant: Uuid, name: &str) -> Result<Option<StoredSecret>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", name AS "name!", value_ct AS "value_ct!: Vec<u8>",
                      value_nonce AS "value_nonce!: Vec<u8>", aead_algo AS "aead_algo!",
                      dek_wrapped AS "dek_wrapped!: Vec<u8>", kek_version AS "kek_version!: i64",
                      created_at AS "created_at!: DateTime<Utc>", updated_at AS "updated_at!: DateTime<Utc>"
               FROM secrets WHERE tenant_id = ? AND name = ?"#,
            tenant,
            name
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| StoredSecret {
            id: r.id,
            name: r.name,
            value_ct: r.value_ct,
            value_nonce: r.value_nonce,
            aead_algo: r.aead_algo,
            dek_wrapped: r.dek_wrapped,
            kek_version: r.kek_version as u32,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }))
    }

    async fn get_secret(&self, id: Uuid, scope: Option<Uuid>) -> Result<Option<StoredSecret>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", name AS "name!", value_ct AS "value_ct!: Vec<u8>",
                      value_nonce AS "value_nonce!: Vec<u8>", aead_algo AS "aead_algo!",
                      dek_wrapped AS "dek_wrapped!: Vec<u8>", kek_version AS "kek_version!: i64",
                      created_at AS "created_at!: DateTime<Utc>", updated_at AS "updated_at!: DateTime<Utc>"
               FROM secrets WHERE id = ?1 AND (?2 IS NULL OR tenant_id = ?2)"#,
            id,
            scope
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| StoredSecret {
            id: r.id,
            name: r.name,
            value_ct: r.value_ct,
            value_nonce: r.value_nonce,
            aead_algo: r.aead_algo,
            dek_wrapped: r.dek_wrapped,
            kek_version: r.kek_version as u32,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }))
    }

    async fn list_secret_names(&self, scope: Option<Uuid>) -> Result<Vec<SecretMeta>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", name AS "name!", kek_version AS "kek_version!: i64",
                      created_at AS "created_at!: DateTime<Utc>", updated_at AS "updated_at!: DateTime<Utc>"
               FROM secrets WHERE (?1 IS NULL OR tenant_id = ?1) ORDER BY name"#,
            scope
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| SecretMeta {
                id: r.id,
                name: r.name,
                kek_version: r.kek_version as u32,
                created_at: r.created_at,
                updated_at: r.updated_at,
            })
            .collect())
    }

    async fn delete_secret(&self, id: Uuid) -> Result<()> {
        sqlx::query!("DELETE FROM secrets WHERE id = ?", id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn insert_kek_version(&self, version: u32, state: &str) -> Result<()> {
        let now = Utc::now();
        let v = version as i64;
        sqlx::query!(
            "INSERT INTO kek_versions (version, state, created_at) VALUES (?, ?, ?)",
            v,
            state,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn list_kek_versions(&self) -> Result<Vec<StoredKekVersion>> {
        let rows = sqlx::query!(
            r#"SELECT version AS "version!: i64", state AS "state!",
                      created_at AS "created_at!: DateTime<Utc>", retired_at AS "retired_at?: DateTime<Utc>"
               FROM kek_versions ORDER BY version"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| StoredKekVersion {
                version: r.version as u32,
                state: r.state,
                created_at: r.created_at,
                retired_at: r.retired_at,
            })
            .collect())
    }

    async fn put_kek_share(&self, version: u32, node_id: Uuid, wrapped_kek: &[u8]) -> Result<()> {
        let v = version as i64;
        sqlx::query!(
            r#"INSERT INTO kek_shares (version, node_id, wrapped_kek) VALUES (?, ?, ?)
               ON CONFLICT(version, node_id) DO UPDATE SET wrapped_kek = excluded.wrapped_kek"#,
            v,
            node_id,
            wrapped_kek
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn get_kek_share(&self, version: u32, node_id: Uuid) -> Result<Option<StoredKekShare>> {
        let v = version as i64;
        let row = sqlx::query!(
            r#"SELECT version AS "version!: i64", node_id AS "node_id!: Uuid",
                      wrapped_kek AS "wrapped_kek!: Vec<u8>", acked_at AS "acked_at?: DateTime<Utc>"
               FROM kek_shares WHERE version = ? AND node_id = ?"#,
            v,
            node_id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| StoredKekShare {
            version: r.version as u32,
            node_id: r.node_id,
            wrapped_kek: r.wrapped_kek,
            acked_at: r.acked_at,
        }))
    }

    async fn upsert_node_key(
        &self,
        node_id: Uuid,
        key_version: u32,
        public_key: &[u8],
        status: &str,
    ) -> Result<()> {
        let now = Utc::now();
        let kv = key_version as i64;
        sqlx::query!(
            r#"INSERT INTO node_keys (node_id, key_version, public_key, status, created_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT(node_id, key_version) DO UPDATE SET
                   public_key = excluded.public_key, status = excluded.status"#,
            node_id,
            kv,
            public_key,
            status,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn list_node_keys(&self) -> Result<Vec<StoredNodeKey>> {
        let rows = sqlx::query!(
            r#"SELECT node_id AS "node_id!: Uuid", key_version AS "key_version!: i64",
                      public_key AS "public_key!: Vec<u8>", status AS "status!",
                      created_at AS "created_at!: DateTime<Utc>", approved_at AS "approved_at?: DateTime<Utc>"
               FROM node_keys"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| StoredNodeKey {
                node_id: r.node_id,
                key_version: r.key_version as u32,
                public_key: r.public_key,
                status: r.status,
                created_at: r.created_at,
                approved_at: r.approved_at,
            })
            .collect())
    }
}

#[async_trait]
impl TenantStore for SqliteStore {
    async fn create_tenant(&self, name: &str) -> Result<Tenant> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query!(
            "INSERT INTO tenants (id, name, created_at) VALUES (?, ?, ?)",
            id,
            name,
            now
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(Tenant {
            id,
            name: name.to_string(),
            created_at: now,
        })
    }

    async fn get_tenant(&self, id: Uuid) -> Result<Option<Tenant>> {
        let row = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", name AS "name!", created_at AS "created_at!: DateTime<Utc>"
               FROM tenants WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.map(|r| Tenant {
            id: r.id,
            name: r.name,
            created_at: r.created_at,
        }))
    }

    async fn list_tenants(&self) -> Result<Vec<Tenant>> {
        let rows = sqlx::query!(
            r#"SELECT id AS "id!: Uuid", name AS "name!", created_at AS "created_at!: DateTime<Utc>"
               FROM tenants ORDER BY name"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        Ok(rows
            .into_iter()
            .map(|r| Tenant {
                id: r.id,
                name: r.name,
                created_at: r.created_at,
            })
            .collect())
    }
}

#[async_trait]
impl ConfigStore for SqliteStore {
    async fn create_db_config(
        &self,
        tenant_id: Uuid,
        engine: DbEngine,
        name: &str,
        host: &str,
        port: u16,
        username: &str,
        password_secret: &str,
        database: &str,
    ) -> Result<SharedDbConfig> {
        let id = Uuid::new_v4();
        let port_i = port as i64;
        match engine {
            DbEngine::PgSql => {
                sqlx::query!(
                    r#"INSERT INTO pgsql_configs (id, name, host, port, username, password_secret, database, tenant_id)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
                    id, name, host, port_i, username, password_secret, database, tenant_id
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            DbEngine::MySql => {
                sqlx::query!(
                    r#"INSERT INTO mysql_configs (id, name, host, port, username, password_secret, database, tenant_id)
                       VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
                    id, name, host, port_i, username, password_secret, database, tenant_id
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
        }
        Ok(SharedDbConfig {
            id,
            engine,
            name: name.to_string(),
            host: host.to_string(),
            port,
            username: username.to_string(),
            password_secret: password_secret.to_string(),
            database: database.to_string(),
            tenant_id,
        })
    }

    async fn get_db_config(&self, id: Uuid, scope: Option<Uuid>) -> Result<Option<SharedDbConfig>> {
        let row = sqlx::query!(
            r#"
            SELECT id AS "id!: Uuid", 'pgsql' AS "engine!: String", name AS "name!", host AS "host!",
                   port AS "port!: i64", username AS "username!", password_secret AS "password_secret!",
                   database AS "database!", tenant_id AS "tenant_id!: Uuid"
            FROM pgsql_configs WHERE id = ?1 AND deleted_at IS NULL AND (?2 IS NULL OR tenant_id = ?2)
            UNION ALL
            SELECT id, 'mysql', name, host, port, username, password_secret, database, tenant_id
            FROM mysql_configs WHERE id = ?1 AND deleted_at IS NULL AND (?2 IS NULL OR tenant_id = ?2)
            "#,
            id,
            scope
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(db)?;
        row.map(|r| {
            Ok(SharedDbConfig {
                id: r.id,
                engine: r.engine.parse()?,
                name: r.name,
                host: r.host,
                port: r.port as u16,
                username: r.username,
                password_secret: r.password_secret,
                database: r.database,
                tenant_id: r.tenant_id,
            })
        })
        .transpose()
    }

    async fn list_db_configs(&self, scope: Option<Uuid>) -> Result<Vec<SharedDbConfig>> {
        let rows = sqlx::query!(
            r#"
            SELECT id AS "id!: Uuid", 'pgsql' AS "engine!: String", name AS "name!", host AS "host!",
                   port AS "port!: i64", username AS "username!", password_secret AS "password_secret!",
                   database AS "database!", tenant_id AS "tenant_id!: Uuid"
            FROM pgsql_configs WHERE deleted_at IS NULL AND (?1 IS NULL OR tenant_id = ?1)
            UNION ALL
            SELECT id, 'mysql', name, host, port, username, password_secret, database, tenant_id
            FROM mysql_configs WHERE deleted_at IS NULL AND (?1 IS NULL OR tenant_id = ?1)
            ORDER BY 3
            "#,
            scope
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.into_iter()
            .map(|r| {
                Ok(SharedDbConfig {
                    id: r.id,
                    engine: r.engine.parse()?,
                    name: r.name,
                    host: r.host,
                    port: r.port as u16,
                    username: r.username,
                    password_secret: r.password_secret,
                    database: r.database,
                    tenant_id: r.tenant_id,
                })
            })
            .collect()
    }

    async fn update_db_config(
        &self,
        id: Uuid,
        name: Option<&str>,
        host: Option<&str>,
        port: Option<u16>,
        username: Option<&str>,
        password_secret: Option<&str>,
        database: Option<&str>,
    ) -> Result<SharedDbConfig> {
        let engine = self
            .get_db_config(id, None)
            .await?
            .ok_or_else(|| ArbiterError::NotFound(format!("db config {id}")))?
            .engine;
        let port_i = port.map(|p| p as i64);
        match engine {
            DbEngine::PgSql => {
                sqlx::query!(
                    r#"UPDATE pgsql_configs SET
                        name = COALESCE(?, name), host = COALESCE(?, host),
                        port = COALESCE(?, port), username = COALESCE(?, username),
                        password_secret = COALESCE(?, password_secret),
                        database = COALESCE(?, database)
                       WHERE id = ? AND deleted_at IS NULL"#,
                    name, host, port_i, username, password_secret, database, id
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
            DbEngine::MySql => {
                sqlx::query!(
                    r#"UPDATE mysql_configs SET
                        name = COALESCE(?, name), host = COALESCE(?, host),
                        port = COALESCE(?, port), username = COALESCE(?, username),
                        password_secret = COALESCE(?, password_secret),
                        database = COALESCE(?, database)
                       WHERE id = ? AND deleted_at IS NULL"#,
                    name, host, port_i, username, password_secret, database, id
                )
                .execute(&self.pool)
                .await
                .map_err(db)?;
            }
        }
        self.get_db_config(id, None)
            .await?
            .ok_or_else(|| ArbiterError::NotFound(format!("db config {id}")))
    }

    async fn delete_db_config(&self, id: Uuid) -> Result<()> {
        let now = Utc::now();
        sqlx::query!(
            "UPDATE pgsql_configs SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
            now,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        sqlx::query!(
            "UPDATE mysql_configs SET deleted_at = ? WHERE id = ? AND deleted_at IS NULL",
            now,
            id
        )
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}
