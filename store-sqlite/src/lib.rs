//! Embedded single-node SQLite backend implementing the arbiter `Store` traits.
//!
//! Design notes:
//! - Uses sqlx's runtime query API (no `query!` macros), so no DATABASE_URL is
//!   needed to build.
//! - Claims are a single `UPDATE ... RETURNING` guarded by `busy_timeout` + WAL,
//!   which serializes writers correctly without the read-then-write deadlock that
//!   explicit deferred transactions hit on SQLite.
//! - Leadership uses a lease row, so multiple processes sharing one file elect a
//!   single scheduler; a lone process simply always wins.

use std::str::FromStr;

use arbiter_core::{
    ApiStore, ArbiterError, JobRun, JobRunState, JobSpec, JobStore, MisfirePolicy, Result, RunStore,
    RunnerConfig, Store, User, UserRole, WorkerRecord, WorkerStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteRow};
use sqlx::{QueryBuilder, Row, SqlitePool};
use uuid::Uuid;

fn db<E: std::fmt::Display>(e: E) -> ArbiterError {
    ArbiterError::DatabaseError(e.to_string())
}

const SCHEMA: &str = r#"
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
"#;

/// Columns selected to reconstruct a `JobRun`.
const RUN_COLS: &str =
    "id, job_id, scheduled_for, state, worker_id, exit_code, started_at, finished_at, output, error_output";

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
}

impl Store for SqliteStore {}

const JOB_SELECT: &str = "SELECT j.id, j.name, j.schedule_cron, j.enabled, j.runner_type, \
    j.max_concurrency, j.misfire_policy, s.command AS command, s.working_dir AS working_dir \
    FROM jobs j LEFT JOIN job_runner_shell s ON s.job_id = j.id WHERE j.deleted_at IS NULL";

fn job_spec_from_row(row: &SqliteRow) -> Result<JobSpec> {
    let runner_type: String = row.try_get("runner_type").map_err(db)?;
    let runner_cfg = match runner_type.as_str() {
        "shell" => RunnerConfig::Shell {
            command: row
                .try_get::<Option<String>, _>("command")
                .map_err(db)?
                .unwrap_or_default(),
            working_dir: row.try_get("working_dir").map_err(db)?,
        },
        other => {
            return Err(ArbiterError::ExecutionError(format!(
                "runner '{other}' not supported in the sqlite backend yet"
            )));
        }
    };
    let misfire: String = row.try_get("misfire_policy").map_err(db)?;
    Ok(JobSpec {
        id: row.try_get("id").map_err(db)?,
        name: row.try_get("name").map_err(db)?,
        schedule_cron: row.try_get("schedule_cron").map_err(db)?,
        enabled: row.try_get("enabled").map_err(db)?,
        runner_cfg,
        max_concurrency: row.try_get::<i64, _>("max_concurrency").map_err(db)? as u32,
        misfire_policy: MisfirePolicy::from_str(&misfire)?,
    })
}

fn job_run_from_row(row: &SqliteRow) -> Result<JobRun> {
    let state: String = row.try_get("state").map_err(db)?;
    Ok(JobRun {
        id: row.try_get("id").map_err(db)?,
        job_id: row.try_get("job_id").map_err(db)?,
        scheduled_for: row.try_get("scheduled_for").map_err(db)?,
        state: JobRunState::from_str(&state)?,
        worker_id: row.try_get("worker_id").map_err(db)?,
        exit_code: row
            .try_get::<Option<i64>, _>("exit_code")
            .map_err(db)?
            .map(|v| v as i32),
        started_at: row.try_get("started_at").map_err(db)?,
        finished_at: row.try_get("finished_at").map_err(db)?,
        snapshot: None,
        output: row.try_get("output").map_err(db)?,
        error_output: row.try_get("error_output").map_err(db)?,
    })
}

fn worker_from_row(row: &SqliteRow) -> Result<WorkerRecord> {
    Ok(WorkerRecord {
        id: row.try_get("id").map_err(db)?,
        display_name: row.try_get("display_name").map_err(db)?,
        hostname: row.try_get("hostname").map_err(db)?,
        last_seen: row.try_get("last_seen").map_err(db)?,
        capacity: row.try_get::<i64, _>("capacity").map_err(db)? as u32,
        restart_count: row.try_get::<i64, _>("restart_count").map_err(db)? as u32,
        version: row.try_get("version").map_err(db)?,
    })
}

fn user_from_row(row: &SqliteRow) -> Result<User> {
    let role: String = row.try_get("role").map_err(db)?;
    Ok(User {
        id: row.try_get("id").map_err(db)?,
        username: row.try_get("username").map_err(db)?,
        password_hash: row.try_get("password_hash").map_err(db)?,
        role: UserRole::from_str(&role)?,
        created_at: row.try_get("created_at").map_err(db)?,
    })
}

#[async_trait]
impl JobStore for SqliteStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>> {
        let sql = format!("{JOB_SELECT} AND j.enabled = 1 AND j.schedule_cron IS NOT NULL");
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await.map_err(db)?;
        rows.iter().map(job_spec_from_row).collect()
    }

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool> {
        let res = sqlx::query(
            "INSERT OR IGNORE INTO job_runs (id, job_id, scheduled_for, state) VALUES (?, ?, ?, 'queued')",
        )
        .bind(Uuid::new_v4())
        .bind(job_id)
        .bind(scheduled_for)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(res.rows_affected() == 1)
    }
}

#[async_trait]
impl RunStore for SqliteStore {
    async fn claim_job_runs(&self, worker_id: Uuid, limit: u32) -> Result<Vec<JobRun>> {
        let now = Utc::now();
        // Single atomic statement: the write lock is held for its whole duration,
        // so concurrent claimers serialize (busy_timeout) and never double-claim.
        let sql = format!(
            "UPDATE job_runs SET state = 'running', worker_id = ?, started_at = ? \
             WHERE id IN ( \
                SELECT jr.id FROM job_runs jr JOIN jobs j ON j.id = jr.job_id \
                WHERE jr.state = 'queued' AND jr.scheduled_for <= ? \
                  AND j.enabled = 1 AND j.deleted_at IS NULL \
                ORDER BY jr.scheduled_for LIMIT ? \
             ) RETURNING {RUN_COLS}"
        );
        let rows = sqlx::query(&sql)
            .bind(worker_id)
            .bind(now)
            .bind(now)
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.iter().map(job_run_from_row).collect()
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
        sqlx::query(
            "UPDATE job_runs SET state = ?, exit_code = ?, output = ?, error_output = ?, finished_at = ? WHERE id = ?",
        )
        .bind(new_state.to_string())
        .bind(exit_code.map(|c| c as i64))
        .bind(output)
        .bind(error_output)
        .bind(finished_at)
        .bind(run_id)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }
}

#[async_trait]
impl WorkerStore for SqliteStore {
    async fn heartbeat(&self, worker: &WorkerRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO workers (id, display_name, hostname, last_seen, capacity, active, restart_count, version) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET last_seen = excluded.last_seen, hostname = excluded.hostname, \
                capacity = excluded.capacity, version = excluded.version, active = 1",
        )
        .bind(worker.id)
        .bind(&worker.display_name)
        .bind(&worker.hostname)
        .bind(worker.last_seen)
        .bind(worker.capacity as i64)
        .bind(worker.restart_count as i64)
        .bind(&worker.version)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn lookup_by_id(&self, id: Uuid) -> Result<Option<(String, u32)>> {
        let row = sqlx::query("SELECT display_name, capacity FROM workers WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        match row {
            Some(r) => Ok(Some((
                r.try_get("display_name").map_err(db)?,
                r.try_get::<i64, _>("capacity").map_err(db)? as u32,
            ))),
            None => Ok(None),
        }
    }

    async fn incr_restart_count(&self, id: Uuid, version: &str) -> Result<u32> {
        let row = sqlx::query(
            "UPDATE workers SET restart_count = restart_count + 1, version = ? WHERE id = ? RETURNING restart_count",
        )
        .bind(version)
        .bind(id)
        .fetch_one(&self.pool)
        .await
        .map_err(db)?;
        Ok(row.try_get::<i64, _>("restart_count").map_err(db)? as u32)
    }

    async fn insert_worker(
        &self,
        id: Uuid,
        display_name: &str,
        hostname: &str,
        version: &str,
        restart_count: u32,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO workers (id, display_name, hostname, last_seen, capacity, active, restart_count, version) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(id)
        .bind(display_name)
        .bind(hostname)
        .bind(Utc::now())
        .bind(4_i64)
        .bind(restart_count as i64)
        .bind(version)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    async fn reclaim_dead_workers_jobs(&self, dead_after_secs: u32) -> Result<u64> {
        let threshold = Utc::now() - Duration::seconds(dead_after_secs as i64);
        let res = sqlx::query(
            "UPDATE job_runs SET state = 'queued', worker_id = NULL, started_at = NULL \
             WHERE state = 'running' AND worker_id IN (SELECT id FROM workers WHERE last_seen < ?)",
        )
        .bind(threshold)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(res.rows_affected())
    }

    async fn am_i_leader(&self) -> Result<bool> {
        let now = Utc::now();
        let expires = now + Duration::seconds(10);
        // Take the lease if absent/expired, or renew it if we already hold it.
        let res = sqlx::query(
            "INSERT INTO leader_lease (id, holder, expires_at) VALUES (1, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET holder = excluded.holder, expires_at = excluded.expires_at \
             WHERE leader_lease.holder = ? OR leader_lease.expires_at <= ?",
        )
        .bind(self.node_id)
        .bind(expires)
        .bind(self.node_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        if res.rows_affected() > 0 {
            return Ok(true);
        }
        let holder: Option<Uuid> = sqlx::query_scalar("SELECT holder FROM leader_lease WHERE id = 1")
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
        sqlx::query("SELECT 1").execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    async fn get_job(&self, job_id: Uuid) -> Result<JobSpec> {
        let sql = format!("{JOB_SELECT} AND j.id = ?");
        let row = sqlx::query(&sql)
            .bind(job_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        match row {
            Some(r) => job_spec_from_row(&r),
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
        sqlx::query(
            "INSERT INTO jobs (id, name, schedule_cron, enabled, runner_type, max_concurrency, created_at, misfire_policy) \
             VALUES (?, ?, ?, 0, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(name)
        .bind(&schedule_cron)
        .bind(runner_cfg.type_of_str())
        .bind(max_concurrency as i64)
        .bind(Utc::now())
        .bind(misfire_policy.to_string())
        .execute(&self.pool)
        .await
        .map_err(db)?;

        match &runner_cfg {
            RunnerConfig::Shell {
                command,
                working_dir,
            } => {
                sqlx::query(
                    "INSERT INTO job_runner_shell (job_id, command, working_dir) VALUES (?, ?, ?)",
                )
                .bind(id)
                .bind(command)
                .bind(working_dir)
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
        let rows = sqlx::query(JOB_SELECT).fetch_all(&self.pool).await.map_err(db)?;
        rows.iter().map(job_spec_from_row).collect()
    }

    async fn list_recent_runs(
        &self,
        limit: Option<u32>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
        by_worker_id: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        let mut qb = QueryBuilder::new(format!("SELECT {RUN_COLS} FROM job_runs WHERE 1 = 1"));
        if let Some(j) = by_job_id {
            qb.push(" AND job_id = ").push_bind(j);
        }
        if let Some(w) = by_worker_id {
            qb.push(" AND worker_id = ").push_bind(w);
        }
        if let Some(b) = before {
            qb.push(" AND scheduled_for < ").push_bind(b);
        }
        if let Some(a) = after {
            qb.push(" AND scheduled_for > ").push_bind(a);
        }
        qb.push(" ORDER BY scheduled_for DESC");
        if let Some(l) = limit {
            qb.push(" LIMIT ").push_bind(l as i64);
        }
        let rows = qb.build().fetch_all(&self.pool).await.map_err(db)?;
        rows.iter().map(job_run_from_row).collect()
    }

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE jobs SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(job_id)
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
        _job_id: Uuid,
        _name: Option<String>,
        _schedule_cron: Option<Option<String>>,
        _runner_cfg: Option<RunnerConfig>,
        _max_concurrency: Option<u32>,
        _misfire_policy: Option<MisfirePolicy>,
    ) -> Result<JobSpec> {
        Err(ArbiterError::ExecutionError(
            "update_job not implemented in the sqlite backend yet".to_string(),
        ))
    }

    async fn delete_job(&self, job_id: Uuid) -> Result<()> {
        sqlx::query("UPDATE jobs SET deleted_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(job_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query("INSERT INTO job_runs (id, job_id, scheduled_for, state) VALUES (?, ?, ?, 'queued')")
            .bind(id)
            .bind(job_id)
            .bind(now)
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
        sqlx::query("UPDATE job_runs SET state = 'cancelled', finished_at = ? WHERE id = ?")
            .bind(Utc::now())
            .bind(run_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>> {
        let rows = sqlx::query(
            "SELECT id, display_name, hostname, last_seen, capacity, restart_count, version FROM workers",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        rows.iter().map(worker_from_row).collect()
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let row = sqlx::query("SELECT id, username, password_hash, role, created_at FROM users WHERE username = ?")
            .bind(username)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        match row {
            Some(r) => user_from_row(&r),
            None => Err(ArbiterError::NotFound(format!("user {username}"))),
        }
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User> {
        let row = sqlx::query("SELECT id, username, password_hash, role, created_at FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)?;
        match row {
            Some(r) => user_from_row(&r),
            None => Err(ArbiterError::NotFound(format!("user {user_id}"))),
        }
    }

    async fn create_user(&self, username: &str, password_hash: &str, role: UserRole) -> Result<User> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query("INSERT INTO users (id, username, password_hash, role, created_at) VALUES (?, ?, ?, ?, ?)")
            .bind(id)
            .bind(username)
            .bind(password_hash)
            .bind(role.to_string())
            .bind(now)
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
        let rows = sqlx::query("SELECT id, username, password_hash, role, created_at FROM users")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        rows.iter().map(user_from_row).collect()
    }

    async fn delete_user(&self, user_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<()> {
        sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
            .bind(password_hash)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db)?;
        Ok(())
    }

    async fn update_user(
        &self,
        _user_id: Uuid,
        _username: Option<&str>,
        _password_hash: Option<&str>,
        _role: Option<UserRole>,
    ) -> Result<User> {
        Err(ArbiterError::ExecutionError(
            "update_user not implemented in the sqlite backend yet".to_string(),
        ))
    }

    async fn count_users(&self) -> Result<u32> {
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(db)?;
        Ok(n as u32)
    }
}
