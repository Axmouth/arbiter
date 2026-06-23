use std::{collections::HashMap, num::TryFromIntError};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use arbiter_core::*;
use sqlx::{Pool, Postgres};
use uuid::Uuid;

pub struct PgStore {
    pool: Pool<Postgres>,
    node_id: uuid::Uuid,
}

impl Store for PgStore {}

// TODO: when to delete runs for job id:
// TODO: Change -> Action
// TODO: Job disabled -> Delete queued runs
// TODO: Schedule changed -> Delete queued runs
// TODO: runner_type changed -> Delete queued runs
// TODO: runner config changed -> Delete queued runs
// TODO: job deleted -> Delete all runs
// TODO: misfire policy changed	-> Optional: likely delete queued
// TODO: env vars changed -> Delete queued runs

pub trait UnwrapHelper<T> {
    fn expected_value(self, field: impl AsRef<str>) -> Result<T>;
}

impl<T> UnwrapHelper<T> for Option<T> {
    fn expected_value(self, field: impl AsRef<str>) -> Result<T> {
        self.ok_or_else(|| {
            let msg = format!("Missing field {}", field.as_ref());
            tracing::error!("configuration error: {msg}");
            ArbiterError::DatabaseError(msg)
        })
    }
}

impl PgStore {
    pub async fn new(url: &str) -> Result<Self> {
        let pool = Pool::<Postgres>::connect(url)
            .await
            .map_err(|e| ArbiterError::DatabaseError(e.to_string()))?;
        // optional: run migrations
        // sqlx::migrate!().run(&pool).await?;
        Ok(Self {
            pool,
            node_id: uuid::Uuid::new_v4(),
        })
    }

    pub fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    /// Fire a best-effort change notification on a Postgres channel (no payload).
    async fn pg_notify_channel(&self, channel: &str) {
        let _ = sqlx::query("SELECT pg_notify($1, '')")
            .bind(channel)
            .execute(&self.pool)
            .await;
    }

    /// Resolve on the next notification for a channel. On a connection/subscribe failure
    /// it returns `pending` so the caller's backstop poll drives instead of hot-looping.
    async fn pg_await_channel(&self, channel: &str) {
        match sqlx::postgres::PgListener::connect_with(&self.pool).await {
            Ok(mut listener) => {
                if listener.listen(channel).await.is_ok() {
                    let _ = listener.recv().await;
                } else {
                    std::future::pending::<()>().await
                }
            }
            Err(_) => std::future::pending::<()>().await,
        }
    }

    pub async fn new_dev_pool(&self) -> sqlx::Result<Pool<Postgres>> {
        let pool =
            Pool::<Postgres>::connect("postgres://arbiter:arbiter@localhost:5432/arbiter").await?;
        Ok(pool)
    }

    async fn load_env_for_job(
        &self,
        tx: &mut sqlx::Transaction<'_, Postgres>,
        job_id: Uuid,
    ) -> Result<Vec<(String, String)>> {
        let rows = sqlx::query!(
            r#"
            SELECT key, value
            FROM job_env_vars
            WHERE job_id = $1
            "#,
            job_id
        )
        .fetch_all(&mut **tx)
        .await?;

        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    async fn invalidate_queued_runs(&self, job_id: Uuid) -> Result<u64> {
        let res = sqlx::query!(
            r#"
            DELETE FROM job_runs
            WHERE job_id = $1
            AND state = 'queued'
            "#,
            job_id
        )
        .execute(&self.pool)
        .await?;

        Ok(res.rows_affected() as u64)
    }

    async fn load_jobspec_full(&self, job_id: Uuid, scope: Option<Uuid>) -> Result<JobSpec> {
        // identical structure + joins as list_enabled_cron_jobs
        // but filtering WHERE j.id = $1 (and an optional tenant scope)
        // reuse Option::expected_value for required runner fields

        let rows = sqlx::query!(
            r#"
            SELECT
                j.id,
                j.name,
                j.schedule_cron,
                j.enabled,
                j.runner_type,
                j.max_concurrency,
                j.misfire_policy,
                j.max_attempts,
                j.backoff_strategy,
                j.backoff_base_secs,
                j.backoff_cap_secs,

                s.command        AS "shell_command?",
                s.working_dir    AS "shell_working_dir?",

                h.method         AS "http_method?",
                h.url            AS "http_url?",
                h.headers        AS "http_headers?",
                h.body           AS "http_body?",
                h.timeout_sec    AS "http_timeout_sec?",

                pg.config_id     AS "pg_config_id?",
                pg.query         AS "pg_query?",
                pg.timeout_sec   AS "pg_timeout_sec?",

                my.config_id     AS "my_config_id?",
                my.query         AS "my_query?",
                my.timeout_sec   AS "my_timeout_sec?",

                py.module        AS "py_module?",
                py.class_name    AS "py_class_name?",
                py.timeout_sec   AS "py_timeout_sec?",

                nd.module        AS "node_module?",
                nd.function_name AS "node_function_name?",
                nd.timeout_sec   AS "node_timeout_sec?"

            FROM jobs j
            LEFT JOIN job_runner_shell  s   ON s.job_id  = j.id
            LEFT JOIN job_runner_http   h   ON h.job_id  = j.id
            LEFT JOIN job_runner_pgsql  pg  ON pg.job_id = j.id
            LEFT JOIN job_runner_mysql  my  ON my.job_id = j.id
            LEFT JOIN job_runner_python py  ON py.job_id = j.id
            LEFT JOIN job_runner_node   nd  ON nd.job_id = j.id
            WHERE j.id = $1
              AND j.deleted_at IS NULL
              AND ($2::uuid IS NULL OR j.tenant_id = $2)
            "#,
            job_id,
            scope
        )
        .fetch_optional(&self.pool)
        .await
        .inspect_err(|e| {
            eprintln!("{:#?}", e.as_database_error());
            eprintln!("{:#?}", e);
            eprintln!("{}", e);
        })?;

        let r = rows.ok_or_else(|| ArbiterError::NotFound(format!("job {} not found", job_id)))?;

        let runner_cfg = match r.runner_type.as_str() {
            "shell" => {
                let command = r.shell_command.expected_value("shell_command")?;

                RunnerConfig::Shell {
                    command,
                    working_dir: r.shell_working_dir,
                }
            }
            "http" => {
                let method = r.http_method.expected_value("http_method")?;
                let url = r.http_url.expected_value("http_url")?;

                let headers = match r.http_headers {
                    Some(headers_json) => {
                        let v: serde_json::Value = headers_json;
                        match serde_json::from_value(v) {
                            Ok(map) => Some(map),
                            Err(e) => {
                                tracing::error!(
                                    "invalid HTTP headers JSON for job {}: {}",
                                    r.id,
                                    e
                                );
                                return Err(ArbiterError::DatabaseError(
                                    "invalid http headers json".to_string(),
                                ));
                            }
                        }
                    }
                    None => None,
                };

                RunnerConfig::Http {
                    method,
                    url,
                    headers,
                    body: r.http_body,
                    timeout_sec: r.http_timeout_sec.map(|x| x as u32),
                }
            }
            "pgsql" => {
                let config_id = r.pg_config_id.expected_value("pg_config_id")?;
                let query = r.pg_query.expected_value("pg_query")?;

                RunnerConfig::PgSql {
                    config_id,
                    query,
                    timeout_sec: r.pg_timeout_sec.map(|x| x as u32),
                }
            }
            "mysql" => {
                let config_id = r.my_config_id.expected_value("my_config_id")?;
                let query = r.my_query.expected_value("my_query")?;

                RunnerConfig::MySql {
                    config_id,
                    query,
                    timeout_sec: r.my_timeout_sec.map(|x| x as u32),
                }
            }
            "python" => {
                let module = r.py_module.expected_value("py_module")?;
                let class_name = r.py_class_name.expected_value("py_class_name")?;

                RunnerConfig::Python {
                    module,
                    class_name,
                    timeout_sec: r.py_timeout_sec.map(|x| x as u32),
                }
            }
            "node" => {
                let module = r.node_module.expected_value("node_module")?;
                let function_name = r.node_function_name.expected_value("node_function_name")?;

                RunnerConfig::Node {
                    module,
                    function_name,
                    timeout_sec: r.node_timeout_sec.map(|x| x as u32),
                }
            }
            other => {
                tracing::error!("unknown runner_type '{}' for job {}", other, r.id);
                return Err(ArbiterError::DatabaseError(format!(
                    "unknown runner_type '{}' for job {}",
                    other, r.id
                )));
            }
        };
        let misfire_policy = r.misfire_policy.parse()?;

        Ok(JobSpec {
            id: r.id,
            name: r.name,
            schedule_cron: r.schedule_cron,
            enabled: r.enabled,
            runner_cfg,
            max_concurrency: r.max_concurrency as u32,
            misfire_policy,
            max_attempts: r.max_attempts as u32,
            backoff_strategy: r.backoff_strategy.parse()?,
            backoff_base_secs: r.backoff_base_secs as u32,
            backoff_cap_secs: r.backoff_cap_secs as u32,
        })
    }

    async fn build_snapshot_for_job(
        &self,
        tx: &mut sqlx::Transaction<'_, Postgres>,
        job_id: Uuid,
    ) -> Result<ExecutableConfigSnapshot> {
        let rec = sqlx::query!(
            r#"
            SELECT
                j.id,
                j.name,
                j.runner_type,

                -- shell
                s.command        AS "shell_command?",
                s.working_dir    AS "shell_working_dir?",

                -- http
                h.method         AS "http_method?",
                h.url            AS "http_url?",
                h.headers        AS "http_headers?",
                h.body           AS "http_body?",
                h.timeout_sec    AS "http_timeout_sec?",

                -- pgsql job-side
                pg.config_id     AS "pg_config_id?",
                pg.query         AS "pg_query?",
                pg.timeout_sec   AS "pg_timeout_sec?",

                -- pgsql config-side
                pgcfg.name       AS "pg_config_name?",
                pgcfg.host       AS "pg_host?",
                pgcfg.port       AS "pg_port?",
                pgcfg.username   AS "pg_username?",
                pgcfg.password_secret AS "pg_password_secret?",
                pgcfg.database   AS "pg_database?",
                pgcfg.deleted_at AS "pg_deleted_at?",

                -- mysql job-side
                my.config_id     AS "my_config_id?",
                my.query         AS "my_query?",
                my.timeout_sec   AS "my_timeout_sec?",

                -- mysql config-side
                mycfg.name       AS "my_config_name?",
                mycfg.host       AS "my_host?",
                mycfg.port       AS "my_port?",
                mycfg.username   AS "my_username?",
                mycfg.password_secret AS "my_password_secret?",
                mycfg.database   AS "my_database?",
                mycfg.deleted_at AS "my_deleted_at?",

                -- python
                py.module        AS "py_module?",
                py.class_name    AS "py_class_name?",
                py.timeout_sec   AS "py_timeout_sec?",

                -- node
                nd.module        AS "node_module?",
                nd.function_name AS "node_function_name?",
                nd.timeout_sec   AS "node_timeout_sec?"

            FROM jobs j
            LEFT JOIN job_runner_shell   s    ON s.job_id   = j.id
            LEFT JOIN job_runner_http    h    ON h.job_id   = j.id
            LEFT JOIN job_runner_pgsql   pg   ON pg.job_id  = j.id
            LEFT JOIN pgsql_configs      pgcfg ON pgcfg.id  = pg.config_id
            LEFT JOIN job_runner_mysql   my   ON my.job_id  = j.id
            LEFT JOIN mysql_configs      mycfg ON mycfg.id  = my.config_id
            LEFT JOIN job_runner_python  py   ON py.job_id  = j.id
            LEFT JOIN job_runner_node    nd   ON nd.job_id  = j.id
            WHERE j.id = $1
              AND j.deleted_at IS NULL
            "#,
            job_id
        )
        .fetch_optional(&mut **tx)
        .await?;

        let rec = match rec {
            Some(r) => r,
            None => {
                return Err(ArbiterError::NotFound(format!(
                    "tried to claim run for non-existent or deleted job {}",
                    job_id
                )));
            }
        };

        // Load env vars once
        let env = HashMap::from_iter(self.load_env_for_job(tx, job_id).await?.into_iter());

        match rec.runner_type.as_str() {
            "shell" => {
                let command = rec.shell_command.expected_value("shell_command")?;
                Ok(ExecutableConfigSnapshot {
                    name: None,
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::Shell {
                        command,
                        working_dir: rec.shell_working_dir,
                        env,
                    },
                })
            }

            "http" => {
                let method = rec.http_method.expected_value("http_method")?;
                let url = rec.http_url.expected_value("http_url")?;

                // headers are optional
                let headers = match rec.http_headers {
                    Some(json) => {
                        let v: serde_json::Value = json;
                        match serde_json::from_value(v) {
                            Ok(map) => map,
                            Err(e) => {
                                tracing::error!(
                                    "invalid HTTP headers JSON for job {}: {}",
                                    job_id,
                                    e
                                );
                                return Err(ArbiterError::DatabaseError(
                                    "invalid http headers json".to_string(),
                                ));
                            }
                        }
                    }
                    None => HashMap::new(),
                };

                Ok(ExecutableConfigSnapshot {
                    name: None,
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::Http {
                        method,
                        url,
                        headers,
                        body: rec.http_body,
                        timeout_sec: rec.http_timeout_sec.map(|x| x as u32),
                    },
                })
            }

            "pgsql" => {
                let config_id = rec.pg_config_id.expected_value("pg_config_id")?;
                if rec.pg_deleted_at.is_some() {
                    let msg = format!("pgsql config {} is soft-deleted", config_id);
                    tracing::error!("{msg}");
                    return Err(ArbiterError::DatabaseError(msg));
                }

                let query = rec.pg_query.expected_value("pg_query")?;
                let host = rec.pg_host.expected_value("pg_host")?;
                let port_i32 = rec.pg_port.expected_value("pg_port")?;
                let port: u16 = port_i32
                    .try_into()
                    .map_err(|e: TryFromIntError| ArbiterError::ValidationError(e.to_string()))?;

                let username = rec.pg_username.expected_value("pg_username")?;
                let password_secret = rec
                    .pg_password_secret
                    .expected_value("pg_password_secret")?;
                let database = rec.pg_database.expected_value("pg_database")?;
                let config_name = rec.pg_config_name.expected_value("pg_config_name")?;

                Ok(ExecutableConfigSnapshot {
                    name: Some(config_name),
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::PgSql {
                        host,
                        port,
                        username,
                        password_secret,
                        database,
                        query,
                        timeout_sec: rec.pg_timeout_sec.map(|x| x as u32),
                    },
                })
            }

            "mysql" => {
                let config_id = rec.my_config_id.expected_value("my_config_id")?;
                if rec.my_deleted_at.is_some() {
                    let msg = format!("mysql config {} is soft-deleted", config_id);
                    tracing::error!("{msg}");
                    return Err(ArbiterError::DatabaseError(msg));
                }

                let query = rec.my_query.expected_value("my_query")?;
                let host = rec.my_host.expected_value("my_host")?;
                let port_i32 = rec.my_port.expected_value("my_port")?;
                let port: u16 = port_i32
                    .try_into()
                    .map_err(|e: TryFromIntError| ArbiterError::ValidationError(e.to_string()))?;

                let username = rec.my_username.expected_value("my_username")?;
                let password_secret = rec
                    .my_password_secret
                    .expected_value("my_password_secret")?;
                let database = rec.my_database.expected_value("my_database")?;
                let config_name = rec.my_config_name.expected_value("my_config_name")?;

                Ok(ExecutableConfigSnapshot {
                    name: Some(config_name),
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::MySql {
                        host,
                        port,
                        username,
                        password_secret,
                        database,
                        query,
                        timeout_sec: rec.my_timeout_sec.map(|x| x as u32),
                    },
                })
            }

            "python" => {
                let module = rec.py_module.expected_value("py_module")?;
                let class_name = rec.py_class_name.expected_value("py_class_name")?;

                Ok(ExecutableConfigSnapshot {
                    name: None,
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::Python {
                        module,
                        class_name,
                        timeout_sec: rec.py_timeout_sec.map(|x| x as u32),
                        env,
                    },
                })
            }

            "node" => {
                let module = rec.node_module.expected_value("node_module")?;
                let function_name = rec
                    .node_function_name
                    .expected_value("node_function_name")?;

                Ok(ExecutableConfigSnapshot {
                    name: None,
                    job_name: rec.name,
                    meta: ExecutableConfigSnapshotMeta::Node {
                        module,
                        function_name,
                        timeout_sec: rec.node_timeout_sec.map(|x| x as u32),
                        env,
                    },
                })
            }

            other => {
                tracing::error!(
                    "runner_type '{}' not yet supported in build_snapshot_for_job",
                    other
                );
                Err(ArbiterError::DatabaseError(format!(
                    "runner_type '{}' not yet supported for snapshots",
                    other
                )))
            }
        }
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
            SELECT
                j.id,
                j.name,
                j.schedule_cron,
                j.enabled,
                j.runner_type,
                j.max_concurrency,
                j.misfire_policy,
                j.max_attempts,
                j.backoff_strategy,
                j.backoff_base_secs,
                j.backoff_cap_secs,

                s.command        AS shell_command,
                s.working_dir    AS shell_working_dir,

                h.method         AS http_method,
                h.url            AS http_url,
                h.headers        AS http_headers,
                h.body           AS http_body,
                h.timeout_sec    AS http_timeout_sec,

                pg.config_id     AS pg_config_id,
                pg.query         AS pg_query,
                pg.timeout_sec   AS pg_timeout_sec,

                my.config_id     AS my_config_id,
                my.query         AS my_query,
                my.timeout_sec   AS my_timeout_sec,

                py.module        AS py_module,
                py.class_name    AS py_class_name,
                py.timeout_sec   AS py_timeout_sec,

                nd.module        AS node_module,
                nd.function_name AS node_function_name,
                nd.timeout_sec   AS node_timeout_sec

            FROM jobs j
            LEFT JOIN job_runner_shell  s   ON s.job_id  = j.id
            LEFT JOIN job_runner_http   h   ON h.job_id  = j.id
            LEFT JOIN job_runner_pgsql  pg  ON pg.job_id = j.id
            LEFT JOIN job_runner_mysql  my  ON my.job_id = j.id
            LEFT JOIN job_runner_python py  ON py.job_id = j.id
            LEFT JOIN job_runner_node   nd  ON nd.job_id = j.id
            WHERE j.enabled = TRUE
              AND j.schedule_cron IS NOT NULL
              AND j.deleted_at IS NULL
            "#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ArbiterError::DatabaseError(e.to_string()))?;

        rows.into_iter()
            .map(|r| {
                let runner_cfg = match r.runner_type.as_str() {
                    "shell" => {
                        let command = r.shell_command.expected_value("shell_command")?;

                        RunnerConfig::Shell {
                            command,
                            working_dir: r.shell_working_dir,
                        }
                    }
                    "http" => {
                        let method = r.http_method.expected_value("http_method")?;
                        let url = r.http_url.expected_value("http_url")?;

                        let headers = match r.http_headers {
                            Some(headers_json) => {
                                let v: serde_json::Value = headers_json;
                                match serde_json::from_value(v) {
                                    Ok(map) => Some(map),
                                    Err(e) => {
                                        tracing::error!(
                                            "invalid HTTP headers JSON for job {}: {}",
                                            r.id,
                                            e
                                        );
                                        return Err(ArbiterError::DatabaseError(
                                            "invalid http headers json".to_string(),
                                        ));
                                    }
                                }
                            }
                            None => None,
                        };

                        RunnerConfig::Http {
                            method,
                            url,
                            headers,
                            body: r.http_body,
                            timeout_sec: r.http_timeout_sec.map(|x| x as u32),
                        }
                    }
                    "pgsql" => {
                        let config_id = r.pg_config_id.expected_value("pg_config_id")?;
                        let query = r.pg_query.expected_value("pg_query")?;

                        RunnerConfig::PgSql {
                            config_id,
                            query,
                            timeout_sec: r.pg_timeout_sec.map(|x| x as u32),
                        }
                    }
                    "mysql" => {
                        let config_id = r.my_config_id.expected_value("my_config_id")?;
                        let query = r.my_query.expected_value("my_query")?;

                        RunnerConfig::MySql {
                            config_id,
                            query,
                            timeout_sec: r.my_timeout_sec.map(|x| x as u32),
                        }
                    }
                    "python" => {
                        let module = r.py_module.expected_value("py_module")?;
                        let class_name = r.py_class_name.expected_value("py_class_name")?;

                        RunnerConfig::Python {
                            module,
                            class_name,
                            timeout_sec: r.py_timeout_sec.map(|x| x as u32),
                        }
                    }
                    "node" => {
                        let module = r.node_module.expected_value("node_module")?;
                        let function_name =
                            r.node_function_name.expected_value("node_function_name")?;

                        RunnerConfig::Node {
                            module,
                            function_name,
                            timeout_sec: r.node_timeout_sec.map(|x| x as u32),
                        }
                    }
                    other => {
                        tracing::error!("unknown runner_type '{}' for job {}", other, r.id);
                        return Err(ArbiterError::DatabaseError(format!(
                            "unknown runner_type '{}' for job {}",
                            other, r.id
                        )));
                    }
                };

                let misfire_policy = r.misfire_policy.parse::<MisfirePolicy>().map_err(|e| {
                    tracing::error!(
                        "invalid misfire_policy '{}' on job {}: {}",
                        r.misfire_policy,
                        r.id,
                        e
                    );
                    ArbiterError::InvalidInput(format!("invalid misfire_policy for job {}", r.id))
                })?;

                Ok(JobSpec {
                    id: r.id,
                    name: r.name,
                    schedule_cron: r.schedule_cron,
                    enabled: r.enabled,
                    runner_cfg,
                    max_concurrency: r.max_concurrency as u32,
                    misfire_policy,
                    max_attempts: r.max_attempts as u32,
                    backoff_strategy: r.backoff_strategy.parse()?,
                    backoff_base_secs: r.backoff_base_secs as u32,
                    backoff_cap_secs: r.backoff_cap_secs as u32,
                })
            })
            .collect()
    }

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool> {
        let mut tx = self.pool.begin().await?;

        let res = sqlx::query!(
            r#"
            INSERT INTO job_runs (id, job_id, scheduled_for, state)
            VALUES ($1, $2, $3, 'queued')
            ON CONFLICT (job_id, scheduled_for) DO NOTHING
            "#,
            Uuid::new_v4(), // TODO: Do in db maybe?
            job_id,
            scheduled_for,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(res.rows_affected() == 1)
    }

    async fn job_tenant(&self, job_id: Uuid) -> Result<Option<Uuid>> {
        let rec = sqlx::query!("SELECT tenant_id FROM jobs WHERE id = $1", job_id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(rec.map(|r| r.tenant_id))
    }

    async fn await_jobs_change(&self) {
        self.pg_await_channel("arbiter_jobs").await;
    }
}

#[async_trait]
impl RunStore for PgStore {
    async fn prune_runs(&self, older_than: DateTime<Utc>) -> Result<u64> {
        let res = sqlx::query!(
            r#"
            DELETE FROM job_runs
            WHERE scheduled_for < $1
              AND state IN ('succeeded', 'failed', 'cancelled')
            "#,
            older_than
        )
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    async fn claim_job_runs(&self, worker_id: Uuid, limit: u32) -> Result<Vec<JobRun>> {
        let mut tx = self.pool.begin().await?;

        // Step 1: lock and pick candidate runs
        let candidates = sqlx::query!(
            r#"
            SELECT jr.id, jr.job_id, jr.scheduled_for
            FROM job_runs jr
            JOIN jobs j ON j.id = jr.job_id
            WHERE jr.state = 'queued'
              AND jr.scheduled_for <= now()
              AND j.enabled = TRUE
              AND j.deleted_at IS NULL
            ORDER BY jr.scheduled_for
            FOR UPDATE SKIP LOCKED
            LIMIT $1
            "#,
            limit as i64
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut runs = Vec::with_capacity(candidates.len());

        for c in candidates {
            // Maybe unneeded, reuse existing?
            let snapshot = self.build_snapshot_for_job(&mut tx, c.job_id).await?;

            let snapshot_json = match serde_json::to_value(&snapshot) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("failed to serialize snapshot for job_run {}: {}", c.id, e);
                    return Err(ArbiterError::ExecutionError(
                        "failed to serialize run snapshot".to_string(),
                    ));
                }
            };

            // Step 2: mark as running and store snapshot
            let rec = sqlx::query!(
                r#"
                UPDATE job_runs
                SET state = 'running',
                    worker_id = $2,
                    started_at = now(),
                    config_snapshot = $3
                WHERE id = $1
                RETURNING
                    id,
                    job_id,
                    scheduled_for,
                    state,
                    worker_id,
                    attempt,
                    started_at,
                    finished_at,
                    exit_code
                "#,
                c.id,
                worker_id,
                snapshot_json,
            )
            .fetch_one(&mut *tx)
            .await?;

            let state = rec.state.parse::<JobRunState>().map_err(|e| {
                tracing::error!(
                    "invalid state '{}' in job_runs for id {}: {}",
                    rec.state,
                    rec.id,
                    e
                );
                ArbiterError::DatabaseError(format!(
                    "invalid state '{}' for run {}",
                    rec.state, rec.id
                ))
            })?;

            runs.push(JobRun {
                id: rec.id,
                job_id: rec.job_id,
                scheduled_for: rec.scheduled_for,
                state,
                worker_id: rec.worker_id,
                exit_code: rec.exit_code,
                attempt: rec.attempt as u32,
                started_at: rec.started_at,
                finished_at: rec.finished_at,
                snapshot: Some(snapshot.clone()),
                result_status: None,
                stdout: None,
                stderr: None,
                result: None,
                result_media_type: None,
                error: None,
                error_media_type: None,
            });
        }

        tx.commit().await?;

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
        let state_str = new_state.to_string();
        let status = status.map(|s| s.to_string());

        sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = $2,
                result_status = $3,
                exit_code = $4,
                stdout = $5,
                stderr = $6,
                result = $7,
                result_media_type = $8,
                error = $9,
                error_media_type = $10,
                finished_at = now()
            WHERE id = $1
            "#,
            run_id,
            state_str,
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
        )
        .execute(&self.pool)
        .await?;

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

        sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = 'queued',
                worker_id = NULL,
                started_at = NULL,
                finished_at = NULL,
                attempt = $2,
                scheduled_for = $3,
                result_status = $4,
                exit_code = $5,
                stdout = $6,
                stderr = $7,
                result = $8,
                result_media_type = $9,
                error = $10,
                error_media_type = $11
            WHERE id = $1
            "#,
            run_id,
            attempt as i32,
            scheduled_for,
            status,
            exit_code,
            stdout,
            stderr,
            result,
            result_media_type,
            error,
            error_media_type,
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
            INSERT INTO workers(id, display_name, hostname, last_seen, capacity, version, active)
            VALUES ($1, $2, $3, $4, $5, $6, true)
            ON CONFLICT (id) DO UPDATE
            SET last_seen = EXCLUDED.last_seen,
                hostname = EXCLUDED.hostname,
                capacity = EXCLUDED.capacity,
                version = EXCLUDED.version,
                active = true
        "#,
            worker.id,
            worker.display_name,
            worker.hostname,
            worker.last_seen,
            worker.capacity as i64,
            worker.version,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn incr_restart_count(&self, id: Uuid, version: &str) -> Result<u32> {
        let rec = sqlx::query!(
            r#"
        UPDATE workers
        SET restart_count = restart_count + 1,
            active = true,
            version = $2
        WHERE id = $1
        RETURNING restart_count
        "#,
            id,
            version,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(rec.restart_count as u32)
    }

    async fn insert_worker(
        &self,
        id: Uuid,
        display_name: &str,
        hostname: &str,
        version: &str,
        restart_count: u32,
    ) -> Result<()> {
        sqlx::query!(
            r#"
        INSERT INTO workers(id, display_name, hostname, last_seen, capacity, restart_count, version)
        VALUES ($1, $2, $3, NOW(), 4, $4, $5)
        "#,
            id,
            display_name,
            hostname,
            restart_count as i32,
            version
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn lookup_by_id(&self, id: Uuid) -> Result<Option<(String, u32)>> {
        let rec = sqlx::query!(
            "SELECT display_name, restart_count FROM workers WHERE id=$1",
            id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(rec.map(|r| (r.display_name, r.restart_count as u32)))
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

    async fn am_i_leader(&self) -> Result<bool> {
        // Lease-row election: stable per node (unlike a pooled advisory lock, which is
        // session-scoped and can flip with connection routing) and TTL-based failover.
        let now = Utc::now();
        let expires = now + Duration::seconds(10);
        let res = sqlx::query!(
            r#"
            INSERT INTO leader_lease (id, holder, expires_at)
            VALUES (1, $1, $2)
            ON CONFLICT (id) DO UPDATE
            SET holder = EXCLUDED.holder, expires_at = EXCLUDED.expires_at
            WHERE leader_lease.holder = $1 OR leader_lease.expires_at <= $3
            "#,
            self.node_id,
            expires,
            now
        )
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(true);
        }
        let holder = sqlx::query_scalar!(r#"SELECT holder FROM leader_lease WHERE id = 1"#)
            .fetch_optional(&self.pool)
            .await?
            .flatten();
        Ok(holder == Some(self.node_id))
    }
}

#[async_trait]
impl ApiStore for PgStore {
    async fn health_check(&self) -> Result<()> {
        sqlx::query!("SELECT 1::int AS health_check")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| ArbiterError::DatabaseError(e.to_string()))?;
        Ok(())
    }

    async fn get_job(&self, job_id: Uuid, scope: Option<Uuid>) -> Result<JobSpec> {
        self.load_jobspec_full(job_id, scope).await
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
        let mut tx = self.pool.begin().await?;

        let runner_type = runner_cfg.type_of_str();
        let new_id = Uuid::new_v4();

        sqlx::query!(
            r#"
        INSERT INTO jobs (
            id, tenant_id, name, schedule_cron,
            runner_type, max_concurrency, misfire_policy,
            max_attempts, backoff_strategy, backoff_base_secs, backoff_cap_secs
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        "#,
            new_id,
            tenant_id,
            name,
            schedule_cron,
            runner_type,
            max_concurrency as i32,
            misfire_policy.to_string(),
            retry.max_attempts as i32,
            retry.backoff_strategy.to_string(),
            retry.backoff_base_secs as i32,
            retry.backoff_cap_secs as i32,
        )
        .execute(&mut *tx)
        .await?;

        match &runner_cfg {
            RunnerConfig::Shell {
                command,
                working_dir,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_shell (job_id, command, working_dir)
                VALUES ($1, $2, $3)
                "#,
                    new_id,
                    command,
                    working_dir.as_deref()
                )
                .execute(&mut *tx)
                .await?;
            }

            RunnerConfig::Http {
                method,
                url,
                headers,
                body,
                timeout_sec,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_http (
                    job_id, method, url, headers, body, timeout_sec
                )
                VALUES ($1, $2, $3, $4, $5, $6)
                "#,
                    new_id,
                    method,
                    url,
                    headers.as_ref().map(|h| serde_json::json!(h)),
                    body.as_deref(),
                    timeout_sec.map(|x| x as i32),
                )
                .execute(&mut *tx)
                .await?;
            }

            RunnerConfig::PgSql {
                config_id,
                query,
                timeout_sec,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_pgsql (
                    job_id, config_id, query, timeout_sec
                )
                VALUES ($1, $2, $3, $4)
                "#,
                    new_id,
                    config_id,
                    query,
                    timeout_sec.map(|x| x as i32)
                )
                .execute(&mut *tx)
                .await?;
            }

            RunnerConfig::MySql {
                config_id,
                query,
                timeout_sec,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_mysql (
                    job_id, config_id, query, timeout_sec
                )
                VALUES ($1, $2, $3, $4)
                "#,
                    new_id,
                    config_id,
                    query,
                    timeout_sec.map(|x| x as i32)
                )
                .execute(&mut *tx)
                .await?;
            }

            RunnerConfig::Python {
                module,
                class_name,
                timeout_sec,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_python (
                    job_id, module, class_name, timeout_sec
                )
                VALUES ($1, $2, $3, $4)
                "#,
                    new_id,
                    module,
                    class_name,
                    timeout_sec.map(|x| x as i32)
                )
                .execute(&mut *tx)
                .await?;
            }

            RunnerConfig::Node {
                module,
                function_name,
                timeout_sec,
            } => {
                sqlx::query!(
                    r#"
                INSERT INTO job_runner_node (
                    job_id, module, function_name, timeout_sec
                )
                VALUES ($1, $2, $3, $4)
                "#,
                    new_id,
                    module,
                    function_name,
                    timeout_sec.map(|x| x as i32)
                )
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit().await?;

        self.pg_notify_channel("arbiter_jobs").await;
        self.load_jobspec_full(new_id, None).await
    }

    async fn list_jobs(&self, scope: Option<Uuid>) -> Result<Vec<JobSpec>> {
        let ids = sqlx::query!(
            r#"
            SELECT id, name, schedule_cron, enabled, max_concurrency, misfire_policy
            FROM jobs
            WHERE deleted_at IS NULL
              AND ($1::uuid IS NULL OR tenant_id = $1)
            ORDER BY created_at DESC
            "#,
            scope
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        // TODO: make single query?
        for row in ids {
            out.push(self.load_jobspec_full(row.id, None).await?);
        }
        Ok(out)
    }

    //TODO: filter based on updated at?
    async fn list_recent_runs(
        &self,
        limit: Option<u32>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
        by_worker_id: Option<Uuid>,
        scope: Option<Uuid>,
    ) -> Result<Vec<JobRun>> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id,
                job_id,
                scheduled_for,
                state,
                worker_id,
                attempt,
                queued_at,
                started_at,
                finished_at,
                exit_code,
                config_snapshot,
                result_status,
                stdout,
                stderr,
                result,
                result_media_type,
                error,
                error_media_type
            FROM job_runs
            WHERE ($1::timestamptz IS NULL OR scheduled_for < $1)
              AND ($2::timestamptz IS NULL OR scheduled_for > $2)
              AND ($4::uuid IS NULL OR job_id = $4)
              AND ($5::uuid IS NULL OR worker_id = $5)
              AND ($6::uuid IS NULL OR job_id IN (SELECT id FROM jobs WHERE tenant_id = $6))
            ORDER BY scheduled_for DESC
            LIMIT COALESCE($3::BIGINT, 9223372036854775807)
            "#,
            before,
            after,
            limit.map(i64::from),
            by_job_id,
            by_worker_id,
            scope,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());

        for r in rows {
            let state = r.state.parse::<JobRunState>().map_err(|e| {
                tracing::error!(
                    "invalid state '{}' in job_runs for id {}: {}",
                    r.state,
                    r.id,
                    e
                );
                ArbiterError::DatabaseError(format!("invalid state '{}' for run {}", r.state, r.id))
            })?;

            let snapshot = match r.config_snapshot {
                Some(config_snapshot) => {
                    match serde_json::from_value::<ExecutableConfigSnapshot>(config_snapshot) {
                        Ok(snap) => Some(snap),
                        Err(e) => {
                            tracing::error!(
                                "Error deserializing jon run snapshot, for run {} : {}",
                                r.id,
                                e
                            );
                            continue;
                        }
                    }
                }
                None => None,
            };

            let result_status = match r.result_status {
                Some(s) => Some(s.parse::<ResultStatus>()?),
                None => None,
            };

            out.push(JobRun {
                id: r.id,
                job_id: r.job_id,
                scheduled_for: r.scheduled_for,
                state,
                worker_id: r.worker_id,
                exit_code: r.exit_code,
                attempt: r.attempt as u32,
                started_at: r.started_at,
                finished_at: r.finished_at,
                snapshot,
                result_status,
                stdout: r.stdout,
                stderr: r.stderr,
                result: r.result,
                result_media_type: r.result_media_type,
                error: r.error,
                error_media_type: r.error_media_type,
            });
        }

        Ok(out)
    }

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()> {
        if !enabled {
            self.invalidate_queued_runs(job_id).await?;
        }
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

        self.pg_notify_channel("arbiter_jobs").await;
        Ok(())
    }

    async fn enable_job(&self, job_id: Uuid) -> Result<()> {
        self.set_job_enabled(job_id, true).await
    }

    async fn disable_job(&self, job_id: Uuid) -> Result<()> {
        self.set_job_enabled(job_id, false).await
    }

    // TODO: when certain fields change(schedule, command, runner, etc). Delete and replan all upcoming runs
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
        let mut tx = self.pool.begin().await?;

        let schedule_specified = schedule_cron.is_some();
        let misfire_policy_specified = misfire_policy.is_some();
        let runner_cfg_specified = runner_cfg.is_some();
        let invalidate = schedule_specified
            || misfire_policy_specified
            || runner_cfg_specified;
        let schedule_val = schedule_cron.unwrap_or(None);

        // Update core job metadata
        let updated = sqlx::query!(
            r#"
            UPDATE jobs
            SET
                name = COALESCE($2, name),
                schedule_cron = CASE
                    WHEN $6 = FALSE THEN schedule_cron
                    ELSE $3::text
                END,
                max_concurrency = COALESCE($4, max_concurrency),
                misfire_policy = COALESCE($5, misfire_policy),
                max_attempts = COALESCE($7, max_attempts),
                backoff_strategy = COALESCE($8, backoff_strategy),
                backoff_base_secs = COALESCE($9, backoff_base_secs),
                backoff_cap_secs = COALESCE($10, backoff_cap_secs)
            WHERE id = $1 AND deleted_at IS NULL
            RETURNING runner_type
            "#,
            job_id,
            name,
            schedule_val,
            max_concurrency.map(|x| x as i32),
            misfire_policy.map(|x| x.to_string()),
            schedule_specified,
            retry.map(|r| r.max_attempts as i32),
            retry.map(|r| r.backoff_strategy.to_string()),
            retry.map(|r| r.backoff_base_secs as i32),
            retry.map(|r| r.backoff_cap_secs as i32),
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(old) = updated else {
            return Err(ArbiterError::NotFound(format!("job {} not found", job_id)));
        };

        // If runner config replaced
        if let Some(new_cfg) = runner_cfg {

            // Remove old runner row
            match old.runner_type.as_str() {
                "shell" => {
                    sqlx::query!("DELETE FROM job_runner_shell WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                "http" => {
                    sqlx::query!("DELETE FROM job_runner_http WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                "pgsql" => {
                    sqlx::query!("DELETE FROM job_runner_pgsql WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                "mysql" => {
                    sqlx::query!("DELETE FROM job_runner_mysql WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                "python" => {
                    sqlx::query!("DELETE FROM job_runner_python WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                "node" => {
                    sqlx::query!("DELETE FROM job_runner_node WHERE job_id=$1", job_id)
                        .execute(&mut *tx)
                        .await?;
                }
                _ => {
                    return Err(ArbiterError::DatabaseError(format!(
                        "unknown runner_type '{}' for job {}",
                        old.runner_type, job_id
                    )));
                }
            }

            // Insert new config row
            let cfg = new_cfg; // shadow for match
            let new_type = cfg.type_of_str();

            sqlx::query!(
                "UPDATE jobs SET runner_type=$2 WHERE id=$1",
                job_id,
                new_type
            )
            .execute(&mut *tx)
            .await?;

            // Use create_job match arms as template
            match cfg {
                RunnerConfig::Shell {
                    command,
                    working_dir,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_shell(job_id,command,working_dir)
                       VALUES ($1,$2,$3)"#,
                        job_id,
                        command,
                        working_dir
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                RunnerConfig::Http {
                    method,
                    url,
                    headers,
                    body,
                    timeout_sec,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_http(job_id,method,url,headers,body,timeout_sec)
                       VALUES ($1,$2,$3,$4,$5,$6)"#,
                        job_id,
                        method,
                        url,
                        headers.as_ref().map(|h| serde_json::json!(h)),
                        body,
                        timeout_sec.map(|x| x as i32)
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                RunnerConfig::PgSql {
                    config_id,
                    query,
                    timeout_sec,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_pgsql(job_id,config_id,query,timeout_sec)
                       VALUES ($1,$2,$3,$4)"#,
                        job_id,
                        config_id,
                        query,
                        timeout_sec.map(|x| x as i32)
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                RunnerConfig::MySql {
                    config_id,
                    query,
                    timeout_sec,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_mysql(job_id,config_id,query,timeout_sec)
                       VALUES ($1,$2,$3,$4)"#,
                        job_id,
                        config_id,
                        query,
                        timeout_sec.map(|x| x as i32)
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                RunnerConfig::Python {
                    module,
                    class_name,
                    timeout_sec,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_python(job_id,module,class_name,timeout_sec)
                       VALUES ($1,$2,$3,$4)"#,
                        job_id,
                        module,
                        class_name,
                        timeout_sec.map(|x| x as i32)
                    )
                    .execute(&mut *tx)
                    .await?;
                }
                RunnerConfig::Node {
                    module,
                    function_name,
                    timeout_sec,
                } => {
                    sqlx::query!(
                        r#"INSERT INTO job_runner_node(job_id,module,function_name,timeout_sec)
                       VALUES ($1,$2,$3,$4)"#,
                        job_id,
                        module,
                        function_name,
                        timeout_sec.map(|x| x as i32)
                    )
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }

        if invalidate {
            self.invalidate_queued_runs(job_id).await?;
        }

        tx.commit().await?;

        self.load_jobspec_full(job_id, None).await
    }

    async fn delete_job(&self, job_id: Uuid) -> Result<()> {
        self.invalidate_queued_runs(job_id).await?;

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

        self.pg_notify_channel("arbiter_jobs").await;
        Ok(())
    }

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun> {
        let mut tx = self.pool.begin().await?;

        // Validate job exists + resolve config
        let snapshot = self.build_snapshot_for_job(&mut tx, job_id).await?;

        let snapshot_json = serde_json::to_value(&snapshot)
            .map_err(|e| ArbiterError::ExecutionError(e.to_string()))?;

        let id = Uuid::new_v4();

        let rec = sqlx::query!(
            r#"
        INSERT INTO job_runs(id, job_id, scheduled_for, state, config_snapshot)
        VALUES ($1, $2, now(), 'queued', $3)
        RETURNING id, job_id, scheduled_for, state, worker_id, attempt,
                  started_at, finished_at, exit_code
        "#,
            id,
            job_id,
            snapshot_json,
        )
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(JobRun {
            id: rec.id,
            job_id: rec.job_id,
            scheduled_for: rec.scheduled_for,
            state: rec.state.parse()?,
            worker_id: rec.worker_id,
            exit_code: rec.exit_code,
            attempt: rec.attempt as u32,
            started_at: rec.started_at,
            finished_at: rec.finished_at,
            snapshot: Some(snapshot),
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
        let res = sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = 'cancelled',
                exit_code = NULL
            WHERE id = $1
              AND state IN ('queued')
            "#,
            run_id
        )
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(ArbiterError::ValidationError(format!(
                "Run {} is not in queued state or does not exist",
                run_id
            )));
        }

        Ok(())
    }

    async fn set_job_env(&self, job_id: Uuid, env: HashMap<String, String>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!("DELETE FROM job_env_vars WHERE job_id = $1", job_id)
            .execute(&mut *tx)
            .await?;
        for (key, value) in &env {
            sqlx::query!(
                "INSERT INTO job_env_vars (job_id, key, value) VALUES ($1, $2, $3)",
                job_id,
                key,
                value
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn get_job_env(&self, job_id: Uuid) -> Result<HashMap<String, String>> {
        let rows = sqlx::query!(
            "SELECT key, value FROM job_env_vars WHERE job_id = $1",
            job_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.key, r.value)).collect())
    }

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>> {
        let rows = sqlx::query!(
            r#"
            SELECT id, display_name, hostname, last_seen, capacity, restart_count, version
            FROM workers
            ORDER BY last_seen DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

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

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, tenant_id, created_at
        FROM users
        WHERE id = $1
        "#,
            user_id,
        )
        .fetch_optional(&self.pool)
        .await?;

        let rec = match rec {
            Some(r) => r,
            None => {
                return Err(ArbiterError::NotFound(format!(
                    "User with id {} not found",
                    user_id
                )));
            }
        };

        Ok(User {
            id: rec.id,
            username: rec.username,
            password_hash: rec.password_hash,
            role: rec.role.parse()?,
            tenant_id: rec.tenant_id,
            created_at: rec.created_at,
        })
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, tenant_id, created_at
        FROM users
        WHERE username = $1
        "#,
            username,
        )
        .fetch_optional(&self.pool)
        .await?;

        let rec = match rec {
            Some(r) => r,
            None => {
                return Err(ArbiterError::NotFound(format!(
                    "User with username {} not found",
                    username
                )));
            }
        };

        Ok(User {
            id: rec.id,
            username: rec.username,
            password_hash: rec.password_hash,
            role: rec.role.parse()?,
            tenant_id: rec.tenant_id,
            created_at: rec.created_at,
        })
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
        tenant_id: Option<Uuid>,
    ) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        INSERT INTO users (username, password_hash, role, tenant_id)
        VALUES ($1, $2, $3, $4)
        RETURNING id, username, password_hash, role, tenant_id, created_at
        "#,
            username,
            password_hash,
            role.to_string(),
            tenant_id,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(User {
            id: rec.id,
            username: rec.username,
            password_hash: rec.password_hash,
            role: rec.role.parse()?,
            tenant_id: rec.tenant_id,
            created_at: rec.created_at,
        })
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, tenant_id, created_at
        FROM users
        ORDER BY created_at DESC
        "#
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| User {
                id: r.id,
                username: r.username,
                password_hash: r.password_hash,
                role: r.role.parse().unwrap(),
                tenant_id: r.tenant_id,
                created_at: r.created_at,
            })
            .collect())
    }

    async fn delete_user(&self, user_id: Uuid) -> Result<()> {
        sqlx::query!(
            r#"
        DELETE FROM users
        WHERE id = $1
        "#,
            user_id,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<()> {
        sqlx::query!(
            r#"
        UPDATE users
        SET password_hash = $2
        WHERE id = $1
        "#,
            user_id,
            password_hash,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn update_user(
        &self,
        user_id: Uuid,
        username: Option<&str>,
        password_hash: Option<&str>,
        role: Option<UserRole>,
    ) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        UPDATE users
        SET password_hash = COALESCE($2, password_hash),
            username = COALESCE($3, username),
            role = COALESCE($4, role)
        WHERE id = $1
        RETURNING password_hash, username, role, id, tenant_id, created_at
        "#,
            user_id,
            password_hash,
            username,
            role.map(|r| r.to_string())
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(User {
            id: rec.id,
            username: rec.username,
            created_at: rec.created_at,
            password_hash: rec.password_hash,
            role: rec.role.parse()?,
            tenant_id: rec.tenant_id,
        })
    }

    async fn count_users(&self) -> Result<u32> {
        let rec = sqlx::query!(
            r#"
        SELECT COUNT(*)::bigint AS count
        FROM users
        "#
        )
        .fetch_one(&self.pool)
        .await?;

        let rec_count = rec.count.unwrap_or(0);
        let count: u32 = if let Ok(cnt) = rec_count.try_into() {
            cnt
        } else {
            return Err(ArbiterError::DatabaseError(format!(
                "Too many users {rec_count}"
            )));
        };

        Ok(count)
    }
}

#[async_trait]
impl SettingsStore for PgStore {
    async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query!("SELECT value FROM settings WHERE key = $1", key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|r| r.value))
    }

    async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO settings (key, value, updated_at)
            VALUES ($1, $2, now())
            ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()
            "#,
            key,
            value
        )
        .execute(&self.pool)
        .await?;
        // Wake any listeners (best-effort; the backstop poll covers a missed notify).
        self.pg_notify_channel("arbiter_settings").await;
        Ok(())
    }

    async fn list_settings(&self) -> Result<Vec<Setting>> {
        let rows = sqlx::query!("SELECT key, value FROM settings ORDER BY key")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| Setting {
                key: r.key,
                value: r.value,
            })
            .collect())
    }

    async fn await_settings_change(&self) {
        self.pg_await_channel("arbiter_settings").await;
    }
}

#[async_trait]
impl SecretStore for PgStore {
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
        let rec = sqlx::query!(
            r#"
            INSERT INTO secrets (tenant_id, name, value_ct, value_nonce, aead_algo, dek_wrapped, kek_version)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (tenant_id, name) DO UPDATE SET
                value_ct = $3, value_nonce = $4, aead_algo = $5,
                dek_wrapped = $6, kek_version = $7, updated_at = now()
            RETURNING id
            "#,
            tenant_id,
            name,
            value_ct,
            value_nonce,
            aead_algo,
            dek_wrapped,
            kek_version as i32,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(rec.id)
    }

    async fn get_secret_by_name(&self, tenant: Uuid, name: &str) -> Result<Option<StoredSecret>> {
        let rec = sqlx::query!(
            r#"SELECT id, name, value_ct, value_nonce, aead_algo, dek_wrapped, kek_version,
                      created_at, updated_at
               FROM secrets WHERE tenant_id = $1 AND name = $2"#,
            tenant,
            name
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(rec.map(|r| StoredSecret {
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
        let rec = sqlx::query!(
            r#"SELECT id, name, value_ct, value_nonce, aead_algo, dek_wrapped, kek_version,
                      created_at, updated_at
               FROM secrets WHERE id = $1 AND ($2::uuid IS NULL OR tenant_id = $2)"#,
            id,
            scope
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(rec.map(|r| StoredSecret {
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
            r#"SELECT id, name, kek_version, created_at, updated_at FROM secrets
               WHERE ($1::uuid IS NULL OR tenant_id = $1) ORDER BY name"#,
            scope
        )
        .fetch_all(&self.pool)
        .await?;
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
        sqlx::query!("DELETE FROM secrets WHERE id = $1", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn insert_kek_version(&self, version: u32, state: &str) -> Result<()> {
        sqlx::query!(
            "INSERT INTO kek_versions (version, state) VALUES ($1, $2)",
            version as i32,
            state
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_kek_versions(&self) -> Result<Vec<StoredKekVersion>> {
        let rows = sqlx::query!(
            r#"SELECT version, state, created_at, retired_at FROM kek_versions ORDER BY version"#
        )
        .fetch_all(&self.pool)
        .await?;
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
        sqlx::query!(
            r#"INSERT INTO kek_shares (version, node_id, wrapped_kek) VALUES ($1, $2, $3)
               ON CONFLICT (version, node_id) DO UPDATE SET wrapped_kek = $3"#,
            version as i32,
            node_id,
            wrapped_kek
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_kek_share(&self, version: u32, node_id: Uuid) -> Result<Option<StoredKekShare>> {
        let rec = sqlx::query!(
            r#"SELECT version, node_id, wrapped_kek, acked_at
               FROM kek_shares WHERE version = $1 AND node_id = $2"#,
            version as i32,
            node_id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(rec.map(|r| StoredKekShare {
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
        sqlx::query!(
            r#"INSERT INTO node_keys (node_id, key_version, public_key, status)
               VALUES ($1, $2, $3, $4)
               ON CONFLICT (node_id, key_version) DO UPDATE SET
                   public_key = $3, status = $4"#,
            node_id,
            key_version as i32,
            public_key,
            status
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_node_keys(&self) -> Result<Vec<StoredNodeKey>> {
        let rows = sqlx::query!(
            r#"SELECT node_id, key_version, public_key, status, created_at, approved_at
               FROM node_keys"#
        )
        .fetch_all(&self.pool)
        .await?;
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
impl TenantStore for PgStore {
    async fn create_tenant(&self, name: &str) -> Result<Tenant> {
        let rec = sqlx::query!(
            "INSERT INTO tenants (name) VALUES ($1) RETURNING id, name, created_at",
            name
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(Tenant {
            id: rec.id,
            name: rec.name,
            created_at: rec.created_at,
        })
    }

    async fn get_tenant(&self, id: Uuid) -> Result<Option<Tenant>> {
        let rec = sqlx::query!("SELECT id, name, created_at FROM tenants WHERE id = $1", id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(rec.map(|r| Tenant {
            id: r.id,
            name: r.name,
            created_at: r.created_at,
        }))
    }

    async fn list_tenants(&self) -> Result<Vec<Tenant>> {
        let rows = sqlx::query!("SELECT id, name, created_at FROM tenants ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
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
impl ConfigStore for PgStore {
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
        let port_i = port as i32;
        let id = match engine {
            DbEngine::PgSql => {
                sqlx::query!(
                    r#"INSERT INTO pgsql_configs (name, host, port, username, password_secret, database, tenant_id)
                       VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id"#,
                    name, host, port_i, username, password_secret, database, tenant_id
                )
                .fetch_one(&self.pool)
                .await?
                .id
            }
            DbEngine::MySql => {
                sqlx::query!(
                    r#"INSERT INTO mysql_configs (name, host, port, username, password_secret, database, tenant_id)
                       VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id"#,
                    name, host, port_i, username, password_secret, database, tenant_id
                )
                .fetch_one(&self.pool)
                .await?
                .id
            }
        };
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
            SELECT id AS "id!", 'pgsql' AS "engine!", name AS "name!", host AS "host!",
                   port AS "port!", username AS "username!", password_secret AS "password_secret!",
                   database AS "database!", tenant_id AS "tenant_id!"
            FROM pgsql_configs WHERE id = $1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR tenant_id = $2)
            UNION ALL
            SELECT id, 'mysql', name, host, port, username, password_secret, database, tenant_id
            FROM mysql_configs WHERE id = $1 AND deleted_at IS NULL AND ($2::uuid IS NULL OR tenant_id = $2)
            "#,
            id,
            scope
        )
        .fetch_optional(&self.pool)
        .await?;
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
            SELECT id AS "id!", 'pgsql' AS "engine!", name AS "name!", host AS "host!",
                   port AS "port!", username AS "username!", password_secret AS "password_secret!",
                   database AS "database!", tenant_id AS "tenant_id!"
            FROM pgsql_configs WHERE deleted_at IS NULL AND ($1::uuid IS NULL OR tenant_id = $1)
            UNION ALL
            SELECT id, 'mysql', name, host, port, username, password_secret, database, tenant_id
            FROM mysql_configs WHERE deleted_at IS NULL AND ($1::uuid IS NULL OR tenant_id = $1)
            ORDER BY 3
            "#,
            scope
        )
        .fetch_all(&self.pool)
        .await?;
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
        let port_i = port.map(|p| p as i32);
        match engine {
            DbEngine::PgSql => {
                sqlx::query!(
                    r#"UPDATE pgsql_configs SET
                        name = COALESCE($2, name), host = COALESCE($3, host),
                        port = COALESCE($4, port), username = COALESCE($5, username),
                        password_secret = COALESCE($6, password_secret),
                        database = COALESCE($7, database)
                       WHERE id = $1 AND deleted_at IS NULL"#,
                    id, name, host, port_i, username, password_secret, database
                )
                .execute(&self.pool)
                .await?;
            }
            DbEngine::MySql => {
                sqlx::query!(
                    r#"UPDATE mysql_configs SET
                        name = COALESCE($2, name), host = COALESCE($3, host),
                        port = COALESCE($4, port), username = COALESCE($5, username),
                        password_secret = COALESCE($6, password_secret),
                        database = COALESCE($7, database)
                       WHERE id = $1 AND deleted_at IS NULL"#,
                    id, name, host, port_i, username, password_secret, database
                )
                .execute(&self.pool)
                .await?;
            }
        }
        self.get_db_config(id, None)
            .await?
            .ok_or_else(|| ArbiterError::NotFound(format!("db config {id}")))
    }

    async fn delete_db_config(&self, id: Uuid) -> Result<()> {
        sqlx::query!(
            "UPDATE pgsql_configs SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
            id
        )
        .execute(&self.pool)
        .await?;
        sqlx::query!(
            "UPDATE mysql_configs SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL",
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
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

        sqlx::query("INSERT INTO jobs (id, name, runner_type) VALUES ($1, $2, $3)")
            .bind(uuid::Uuid::new_v4())
            .bind("test")
            .bind("shell")
            .execute(&pool)
            .await
            .unwrap();
    }
}
