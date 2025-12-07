use std::{collections::HashMap, num::TryFromIntError};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dromio_core::*;
use sqlx::{Pool, Postgres};
use uuid::Uuid;

pub struct PgStore {
    pool: Pool<Postgres>,
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
            DromioError::DatabaseError(msg)
        })
    }
}

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

    async fn load_jobspec_full(&self, job_id: Uuid) -> Result<JobSpec> {
        // identical structure + joins as list_enabled_cron_jobs
        // but filtering WHERE j.id = $1
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
            "#,
            job_id
        )
        .fetch_optional(&self.pool)
        .await
        .inspect_err(|e| {
            eprintln!("{:#?}", e.as_database_error());
            eprintln!("{:#?}", e);
            eprintln!("{}", e);
        })?;

        let r = rows.ok_or_else(|| DromioError::NotFound(format!("job {} not found", job_id)))?;

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
                                return Err(DromioError::DatabaseError(
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
                return Err(DromioError::DatabaseError(format!(
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
                return Err(DromioError::NotFound(format!(
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
                                return Err(DromioError::DatabaseError(
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
                    return Err(DromioError::DatabaseError(msg));
                }

                let query = rec.pg_query.expected_value("pg_query")?;
                let host = rec.pg_host.expected_value("pg_host")?;
                let port_i32 = rec.pg_port.expected_value("pg_port")?;
                let port: u16 = port_i32
                    .try_into()
                    .map_err(|e: TryFromIntError| DromioError::ValidationError(e.to_string()))?;

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
                    return Err(DromioError::DatabaseError(msg));
                }

                let query = rec.my_query.expected_value("my_query")?;
                let host = rec.my_host.expected_value("my_host")?;
                let port_i32 = rec.my_port.expected_value("my_port")?;
                let port: u16 = port_i32
                    .try_into()
                    .map_err(|e: TryFromIntError| DromioError::ValidationError(e.to_string()))?;

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
                Err(DromioError::DatabaseError(format!(
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
        .map_err(|e| DromioError::DatabaseError(e.to_string()))?;

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
                                        return Err(DromioError::DatabaseError(
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
                        return Err(DromioError::DatabaseError(format!(
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
                    DromioError::InvalidInput(format!("invalid misfire_policy for job {}", r.id))
                })?;

                Ok(JobSpec {
                    id: r.id,
                    name: r.name,
                    schedule_cron: r.schedule_cron,
                    enabled: r.enabled,
                    runner_cfg,
                    max_concurrency: r.max_concurrency as u32,
                    misfire_policy,
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
}

#[async_trait]
impl RunStore for PgStore {
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
                    return Err(DromioError::ExecutionError(
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
                    started_at,
                    finished_at,
                    exit_code,
                    output,
                    error_output
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
                DromioError::DatabaseError(format!(
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
                started_at: rec.started_at,
                finished_at: rec.finished_at,
                snapshot: Some(snapshot.clone()),
                output: rec.output.map(|v| v.to_string()),
                error_output: rec.error_output,
            });
        }

        tx.commit().await?;

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
        let state_str = new_state.to_string();

        sqlx::query!(
            r#"
            UPDATE job_runs
            SET state = $2,
                exit_code = $3,
                output = $4,
                error_output = $5,
                finished_at = CASE
                    WHEN $2 IN ('succeeded', 'failed', 'cancelled') THEN now()
                    ELSE finished_at
                END
            WHERE id = $1
            "#,
            run_id,
            state_str,
            exit_code,
            output.map(|s| serde_json::Value::String(s)),
            error_output,
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

        Ok(rec.restart_count.unwrap_or(0) as u32)
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

        Ok(rec.map(|r| (r.display_name, r.restart_count.unwrap_or(0) as u32)))
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
        let row = sqlx::query!(
            r#"
            SELECT pg_try_advisory_lock(134037) AS acquired
            "#
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(row.acquired.unwrap_or(false))
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
        self.load_jobspec_full(job_id).await
    }

    async fn create_job(
        &self,
        name: &str,
        schedule_cron: Option<String>,
        runner_cfg: RunnerConfig,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
    ) -> Result<JobSpec> {
        let mut tx = self.pool.begin().await?;

        let runner_type = runner_cfg.type_of_str();
        let new_id = Uuid::new_v4();

        sqlx::query!(
            r#"
        INSERT INTO jobs (
            id, name, schedule_cron,
            runner_type, max_concurrency, misfire_policy
        )
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
            new_id,
            name,
            schedule_cron,
            runner_type,
            max_concurrency as i32,
            misfire_policy.to_string(),
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

        self.load_jobspec_full(new_id).await
    }

    async fn list_jobs(&self) -> Result<Vec<JobSpec>> {
        let ids = sqlx::query!(
            r#"
            SELECT id, name, schedule_cron, enabled, max_concurrency, misfire_policy
            FROM jobs
            WHERE deleted_at IS NULL
            ORDER BY created_at DESC
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::new();
        // TODO: make single query?
        for row in ids {
            out.push(self.load_jobspec_full(row.id).await?);
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
    ) -> Result<Vec<JobRun>> {
        let rows = sqlx::query!(
            r#"
            SELECT
                id,
                job_id,
                scheduled_for,
                state,
                worker_id,
                queued_at,
                started_at,
                finished_at,
                exit_code,
                config_snapshot,
                output,
                error_output
            FROM job_runs
            WHERE ($1::timestamptz IS NULL OR scheduled_for < $1)
              AND ($2::timestamptz IS NULL OR scheduled_for > $2)
              AND ($4::uuid IS NULL OR job_id = $4)
              AND ($5::uuid IS NULL OR worker_id = $5)
            ORDER BY scheduled_for DESC
            LIMIT COALESCE($3::BIGINT, 9223372036854775807)
            "#,
            before,
            after,
            limit.map(i64::from),
            by_job_id,
            by_worker_id,
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
                DromioError::DatabaseError(format!("invalid state '{}' for run {}", r.state, r.id))
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

            out.push(JobRun {
                id: r.id,
                job_id: r.job_id,
                scheduled_for: r.scheduled_for,
                state,
                worker_id: r.worker_id,
                exit_code: r.exit_code,
                started_at: r.started_at,
                finished_at: r.finished_at,
                snapshot,
                output: r.output.map(|v| v.to_string()),
                error_output: r.error_output,
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
    ) -> Result<JobSpec> {
        let mut tx = self.pool.begin().await?;

        let schedule_specified = schedule_cron.is_some();
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
            misfire_policy = COALESCE($5, misfire_policy)
        WHERE id = $1 AND deleted_at IS NULL
        RETURNING runner_type
        "#,
            job_id,
            name,
            schedule_val,
            max_concurrency.map(|x| x as i32),
            misfire_policy.map(|x| x.to_string()),
            schedule_specified,
        )
        .fetch_optional(&mut *tx)
        .await?;

        let Some(old) = updated else {
            return Err(DromioError::NotFound(format!("job {} not found", job_id)));
        };

        let mut invalidate = schedule_specified;

        // If runner config replaced
        if let Some(new_cfg) = runner_cfg {
            invalidate = true;

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
                _ => {}
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

        self.load_jobspec_full(job_id).await
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

        Ok(())
    }

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun> {
        let mut tx = self.pool.begin().await?;

        // Validate job exists + resolve config
        let snapshot = self.build_snapshot_for_job(&mut tx, job_id).await?;

        let snapshot_json = serde_json::to_value(&snapshot)
            .map_err(|e| DromioError::ExecutionError(e.to_string()))?;

        let id = Uuid::new_v4();

        let rec = sqlx::query!(
            r#"
        INSERT INTO job_runs(id, job_id, scheduled_for, state, config_snapshot)
        VALUES ($1, $2, now(), 'queued', $3)
        RETURNING id, job_id, scheduled_for, state, worker_id,
                  started_at, finished_at, exit_code, output, error_output
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
            started_at: rec.started_at,
            finished_at: rec.finished_at,
            snapshot: Some(snapshot),
            output: rec.output.map(|o| o.to_string()),
            error_output: rec.error_output,
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
            return Err(DromioError::ValidationError(format!(
                "Run {} is not in queued state or does not exist",
                run_id
            )));
        }

        Ok(())
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
                restart_count: r.restart_count.unwrap_or(0) as u32,
                version: r.version,
            })
            .collect())
    }

    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, created_at
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
                return Err(DromioError::NotFound(format!(
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
            created_at: rec.created_at,
        })
    }

    async fn get_user_by_username(&self, username: &str) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, created_at
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
                return Err(DromioError::NotFound(format!(
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
            created_at: rec.created_at,
        })
    }

    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
    ) -> Result<User> {
        let rec = sqlx::query!(
            r#"
        INSERT INTO users (username, password_hash, role)
        VALUES ($1, $2, $3)
        RETURNING id, username, password_hash, role, created_at
        "#,
            username,
            password_hash,
            role.to_string(),
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(User {
            id: rec.id,
            username: rec.username,
            password_hash: rec.password_hash,
            role: rec.role.parse()?,
            created_at: rec.created_at,
        })
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let rows = sqlx::query!(
            r#"
        SELECT id, username, password_hash, role, created_at
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
        SET password_hash = COALESCE(password_hash, $2),
            username = COALESCE(username, $3),
            role = COALESCE(role, $4)
        WHERE id = $1
        RETURNING password_hash, username, role, id, created_at
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
            return Err(DromioError::DatabaseError(format!(
                "Too many users {rec_count}"
            )));
        };

        Ok(count)
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
