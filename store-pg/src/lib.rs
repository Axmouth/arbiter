use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dromio_core::*;
use sqlx::{Pool, Postgres};
use uuid::Uuid;

pub struct PgStore {
    pool: Pool<Postgres>,
}

impl Store for PgStore {}

impl PgStore {
    pub async fn new(url: &str) -> Result<Self> {
        let pool = Pool::<Postgres>::connect(url)
            .await
            .map_err(|e| DromioError::DatabaseError(e.to_string()))?;
        // optional: run migrations
        // sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    pub async fn new_dev_pool(&self) -> sqlx::Result<Pool<Postgres>> {
        let pool =
            Pool::<Postgres>::connect("postgres://dromio:dromio@localhost:5432/dromio").await?;
        Ok(pool)
    }

    // Example method:
    pub async fn register_worker(&self, hostname: &str, capacity: i32) -> sqlx::Result<Uuid> {
        let rec = sqlx::query!(
            r#"
            INSERT INTO workers (hostname, last_seen, capacity)
            VALUES ($1, now(), $2)
            RETURNING id
            "#,
            hostname,
            capacity
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(rec.id)
    }

    pub async fn heartbeat(&self, worker_id: Uuid) -> sqlx::Result<()> {
        sqlx::query!(
            r#"
            UPDATE workers
            SET last_seen = now(), active = true
            WHERE id = $1
            "#,
            worker_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl JobStore for PgStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, schedule_cron, enabled, command, max_concurrency, misfire_policy
            FROM jobs
            WHERE enabled = TRUE
              AND schedule_cron IS NOT NULL
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| {
                Ok(JobSpec {
                    id: r.id,
                    name: r.name,
                    schedule_cron: r.schedule_cron,
                    enabled: r.enabled,
                    command: r.command,
                    max_concurrency: r.max_concurrency as u32,
                    misfire_policy: r.misfire_policy.parse()?,
                })
            })
            .collect::<Result<Vec<_>>>()?)
    }

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
        command: &str,
    ) -> Result<bool> {
        let res = sqlx::query!(
            r#"
            INSERT INTO job_runs(id, job_id, scheduled_for, command, state)
            VALUES ($1, $2, $3, $4, 'queued')
            ON CONFLICT (job_id, scheduled_for) DO NOTHING
            "#,
            Uuid::new_v4(),
            job_id,
            scheduled_for,
            command
        )
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected() == 1)
    }
}

#[async_trait]
impl RunStore for PgStore {
    async fn claim_job_runs(&self, worker_id: Uuid, limit: u32) -> Result<Vec<JobRun>> {
        let mut tx = self.pool.begin().await?;

        let rows = sqlx::query!(
            r#"
            WITH cte AS (
                SELECT jr.id
                FROM job_runs jr
                WHERE jr.state = 'queued'
                AND jr.scheduled_for <= now()
                ORDER BY jr.scheduled_for
                FOR UPDATE SKIP LOCKED
                LIMIT $1
            )
            UPDATE job_runs jr
            SET state = 'running',
                worker_id = $2,
                started_at = now(),
                command = j.command         -- snapshot
            FROM jobs j
            WHERE jr.id IN (SELECT id FROM cte)
            AND jr.job_id = j.id
            RETURNING
                jr.id, jr.job_id, jr.scheduled_for, jr.state, jr.worker_id,
                jr.started_at, jr.finished_at, jr.exit_code, jr.command;
            "#,
            limit as i64,
            worker_id
        )
        .fetch_all(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(rows
            .into_iter()
            .map(|r| JobRun {
                id: r.id,
                job_id: r.job_id,
                scheduled_for: r.scheduled_for,
                state: match r.state.as_str() {
                    "queued" => JobRunState::Queued,
                    "running" => JobRunState::Running,
                    "succeeded" => JobRunState::Succeeded,
                    "failed" => JobRunState::Failed,
                    _ => unreachable!(),
                },
                worker_id: r.worker_id,
                exit_code: r.exit_code,
                started_at: r.started_at,
                finished_at: r.finished_at,
                command: r.command,
            })
            .collect())
    }

    async fn update_job_run_state(
        &self,
        run_id: Uuid,
        new_state: JobRunState,
        exit_code: Option<i32>,
    ) -> Result<()> {
        let state_str = match new_state {
            JobRunState::Queued => "queued",
            JobRunState::Running => "running",
            JobRunState::Succeeded => "succeeded",
            JobRunState::Failed => "failed",
            JobRunState::Cancelled => "cancelled",
        };

        sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = $2,
                exit_code = $3,
                finished_at = CASE
                    WHEN $2 IN ('succeeded', 'failed') THEN now()
                    ELSE finished_at
                END
            WHERE id = $1
            "#,
            run_id,
            state_str,
            exit_code
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl WorkerStore for PgStore {
    async fn heartbeat(&self, worker: &WorkerRecord) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO workers(id, hostname, last_seen, capacity)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (id) DO UPDATE
            SET last_seen = EXCLUDED.last_seen,
                hostname = EXCLUDED.hostname,
                capacity = EXCLUDED.capacity
            "#,
            worker.id,
            worker.hostname,
            worker.last_seen,
            worker.capacity as i64
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn reclaim_dead_workers_jobs(&self, dead_after_secs: u32) -> Result<u64> {
        let res = sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = 'queued',
                worker_id = NULL,
                started_at = NULL
            WHERE state = 'running'
              AND worker_id IN (
                SELECT id FROM workers
                WHERE last_seen < now() - ($1::bigint || ' seconds')::interval
              )
            "#,
            dead_after_secs as i64
        )
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected() as u64)
    }
}

#[async_trait]
impl ApiStore for PgStore {
    async fn health_check(&self) -> Result<()> {
        sqlx::query!("SELECT 1::int AS health_check")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| DromioError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_job(&self, job_id: Uuid) -> Result<JobSpec> {
        let rec = sqlx::query!(
            r#"
        SELECT id, name, schedule_cron, enabled, command, max_concurrency, misfire_policy
        FROM jobs
        WHERE id = $1
            AND deleted_at IS NULL
        "#,
            job_id
        )
        .fetch_optional(&self.pool)
        .await?;

        let rec = match rec {
            Some(r) => r,
            None => {
                return Err(DromioError::NotFound(format!(
                    "Job with id {} not found",
                    job_id
                )));
            }
        };

        Ok(JobSpec {
            id: rec.id,
            name: rec.name,
            schedule_cron: rec.schedule_cron,
            enabled: rec.enabled,
            command: rec.command,
            max_concurrency: rec.max_concurrency as u32,
            misfire_policy: rec.misfire_policy.parse()?,
        })
    }

    async fn create_job(
        &self,
        name: &str,
        schedule_cron: Option<String>,
        command: String,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
    ) -> Result<JobSpec> {
        let rec = sqlx::query!(
            r#"
            INSERT INTO jobs (name, schedule_cron, command, max_concurrency, misfire_policy)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, name, schedule_cron, enabled, command, max_concurrency, misfire_policy
            "#,
            name,
            schedule_cron,
            command,
            max_concurrency as i64,
            misfire_policy.to_string(),
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

        Ok(JobSpec {
            id: rec.id,
            name: rec.name,
            schedule_cron: rec.schedule_cron,
            enabled: rec.enabled,
            command: rec.command,
            max_concurrency: rec.max_concurrency as u32,
            misfire_policy: rec.misfire_policy.parse()?,
        })
    }

    async fn list_jobs(&self) -> Result<Vec<JobSpec>> {
        let rows = sqlx::query!(
            r#"
            SELECT id, name, schedule_cron, enabled, command, max_concurrency, misfire_policy
            FROM jobs
            WHERE deleted_at IS NULL
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| {
                Ok(JobSpec {
                    id: r.id,
                    name: r.name,
                    schedule_cron: r.schedule_cron,
                    enabled: r.enabled,
                    command: r.command,
                    max_concurrency: r.max_concurrency as u32,
                    misfire_policy: r.misfire_policy.parse()?,
                })
            })
            .collect::<Result<Vec<_>>>()?)
    }

    async fn list_recent_runs(
        &self,
        limit: u32,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        let rows = sqlx::query!(
            r#"
            SELECT id, job_id, scheduled_for, state, worker_id, queued_at, started_at, finished_at, exit_code, command
            FROM job_runs
            WHERE ($1::timestamptz IS NULL OR scheduled_for < $1)
                AND ($2::timestamptz IS NULL OR scheduled_for > $2)
                AND ($4::uuid IS NULL OR job_id = $4)
            ORDER BY scheduled_for DESC
            LIMIT $3
            "#,
            before,
            after,
            limit as i64,
            by_job_id,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| JobRun {
                id: r.id,
                job_id: r.job_id,
                scheduled_for: r.scheduled_for,
                state: match r.state.as_str() {
                    "queued" => JobRunState::Queued,
                    "running" => JobRunState::Running,
                    "succeeded" => JobRunState::Succeeded,
                    "failed" => JobRunState::Failed,
                    "cancelled" => JobRunState::Cancelled,
                    other => {
                        eprintln!("unknown state {other}, forcing failed");
                        JobRunState::Failed
                    }
                },
                worker_id: r.worker_id,
                exit_code: r.exit_code,
                started_at: r.started_at,
                finished_at: r.finished_at,
                command: r.command,
            })
            .collect())
    }

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE jobs
            SET enabled = $2
            WHERE id = $1
                AND deleted_at IS NULL
            "#,
            job_id,
            enabled
        )
        .execute(&self.pool)
        .await?;

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
        command: Option<String>,
        max_concurrency: Option<u32>,
        misfire_policy: Option<MisfirePolicy>,
    ) -> Result<JobSpec> {
        let schedule_is_specified = schedule_cron.is_some();
        let schedule_value = schedule_cron.flatten();

        let rec = sqlx::query!(
            r#"
            UPDATE jobs
            SET
                name = COALESCE($2, name),
                schedule_cron = CASE
                    WHEN $7 = false THEN schedule_cron     -- outer None: do not change
                    ELSE $3::text                                -- either Some(None)=NULL or Some(value)
                END,
                command = COALESCE($4, command),
                max_concurrency = COALESCE($5, max_concurrency),
                misfire_policy = COALESCE($6, misfire_policy)
            WHERE id = $1
                AND deleted_at IS NULL
            RETURNING id, name, schedule_cron, enabled, command, max_concurrency, misfire_policy
            "#,
            job_id,
            name,
            schedule_value,
            command,
            // TODO: add proper handling for max_concurrency = 0 if needed, plus overflow check
            max_concurrency.map(|x| x as i32),
            misfire_policy.map(|mp| mp.to_string()),
            schedule_is_specified,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(JobSpec {
            id: rec.id,
            name: rec.name,
            schedule_cron: rec.schedule_cron,
            enabled: rec.enabled,
            command: rec.command,
            max_concurrency: rec.max_concurrency as u32,
            misfire_policy: rec.misfire_policy.parse()?,
        })
    }

    async fn delete_job(&self, job_id: Uuid) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE jobs
            SET deleted_at = now()
            WHERE id = $1
            "#,
            job_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn create_adhoc_run(
        &self,
        job_id: Uuid,
        command_override: Option<String>,
    ) -> Result<JobRun> {
        let id = Uuid::new_v4();

        let rec = sqlx::query!(
            r#"
            INSERT INTO job_runs(id, job_id, scheduled_for, command, state)
            SELECT $1, j.id, now(), COALESCE($2, j.command), 'queued'
            FROM jobs j
            WHERE j.id = $3
                AND deleted_at IS NULL
            RETURNING id, job_id, scheduled_for, state, worker_id,
                      queued_at, started_at, finished_at, exit_code, command
            "#,
            id,
            command_override,
            job_id
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(JobRun {
            id: rec.id,
            job_id: rec.job_id,
            scheduled_for: rec.scheduled_for,
            state: JobRunState::Queued,
            worker_id: rec.worker_id,
            exit_code: rec.exit_code,
            started_at: rec.started_at,
            finished_at: rec.finished_at,
            command: rec.command,
        })
    }

    async fn cancel_run(&self, run_id: Uuid) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = 'failed',          -- or add a dedicated 'cancelled'
                finished_at = now(),
                exit_code = NULL
            WHERE id = $1
              AND state IN ('queued', 'running')
            "#,
            run_id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>> {
        let rows = sqlx::query!(
            r#"
            SELECT id, hostname, last_seen, capacity
            FROM workers
            ORDER BY last_seen DESC
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| WorkerRecord {
                id: r.id,
                hostname: r.hostname,
                last_seen: r.last_seen,
                capacity: r.capacity as u32,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;
    use testcontainers::{ContainerAsync, runners::AsyncRunner};
    use testcontainers_modules::postgres;

    async fn setup_postgres_container() -> (ContainerAsync<postgres::Postgres>, String) {
        let container = postgres::Postgres::default().start().await.unwrap();
        let host_port = container.get_host_port_ipv4(5432).await.unwrap();

        (
            container,
            format!(
                "postgres://postgres:postgres@localhost:{}/postgres",
                host_port
            ),
        )
    }

    #[tokio::test]
    async fn test_insert_job() {
        let (_container, url) = setup_postgres_container().await;
        eprintln!("Postgres URL: {}", url);

        // SQLx doesn't create the schema for you, so run migrations:
        sqlx::migrate!("../docker/init")
            .run(&PgPool::connect(&url).await.unwrap())
            .await
            .unwrap();

        // Now you can run your actual code:
        let pool = PgPool::connect(&url).await.unwrap();

        sqlx::query("INSERT INTO jobs (id, name, command) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::new_v4())
            .bind("test")
            .bind("echo hello")
            .execute(&pool)
            .await
            .unwrap();
    }
}
