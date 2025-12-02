use std::{fmt, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum DromioError {
    ExecutionError(String),
    DatabaseError(String),
    NotFound(String),
    InvalidInput(String),
    // other error variants
}

impl From<std::num::ParseIntError> for DromioError {
    fn from(err: std::num::ParseIntError) -> Self {
        DromioError::InvalidInput(err.to_string())
    }
}

impl From<sqlx::Error> for DromioError {
    fn from(err: sqlx::Error) -> Self {
        DromioError::DatabaseError(err.to_string())
    }
}

impl fmt::Display for DromioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DromioError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            DromioError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            DromioError::NotFound(msg) => write!(f, "Not found: {}", msg),
            DromioError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
        }
    }
}

impl std::error::Error for DromioError {}

pub type Result<T> = std::result::Result<T, DromioError>;

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[ts(export)]
pub struct JobSpec {
    pub id: Uuid,
    pub name: String,
    pub schedule_cron: Option<String>, // cron syntax, TODO: use a proper type instead of unsafe String
    pub enabled: bool,
    pub command: String,
    pub max_concurrency: u32,
    pub misfire_policy: MisfirePolicy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum JobRunState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

pub struct SchedulerConfig {
    pub tick_interval_ms: u64,
}

pub struct WorkerConfig {
    pub worker_id: Uuid,
    pub capacity: u32,
    pub hostname: String,
    pub tick_interval_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub dead_after_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MisfirePolicy {
    Skip,
    RunIfLateWithin(Duration),
    RunImmediately,
    Coalesce,
    RunAll, // rarely used
}

impl fmt::Display for MisfirePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MisfirePolicy::Skip => write!(f, "skip"),
            MisfirePolicy::RunIfLateWithin(dur) => write!(f, "run_if_late_within({})", dur),
            MisfirePolicy::RunImmediately => write!(f, "run_immediately"),
            MisfirePolicy::Coalesce => write!(f, "coalesce"),
            MisfirePolicy::RunAll => write!(f, "run_all"),
        }
    }
}

impl FromStr for MisfirePolicy {
    type Err = DromioError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "skip" => Ok(MisfirePolicy::Skip),
            "run_immediately" => Ok(MisfirePolicy::RunImmediately),
            "coalesce" => Ok(MisfirePolicy::Coalesce),
            "run_all" => Ok(MisfirePolicy::RunAll),
            _ if s.starts_with("run_if_late_within(") && s.ends_with(")") => {
                let dur_str = &s["run_if_late_within(".len()..s.len() - 1];
                let dur = dur_str.parse::<u32>().map_err(|_| {
                    DromioError::InvalidInput(format!("Invalid duration: {}", dur_str))
                })?;
                Ok(MisfirePolicy::RunIfLateWithin(Duration::seconds(
                    dur as i64,
                )))
            }
            _ => Err(DromioError::InvalidInput(format!(
                "Unknown misfire policy: {}",
                s
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct JobRun {
    pub id: Uuid,
    pub job_id: Uuid,
    pub scheduled_for: DateTime<Utc>,
    pub state: JobRunState,
    pub worker_id: Option<Uuid>,
    pub exit_code: Option<i32>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub struct WorkerRecord {
    pub id: Uuid,
    pub hostname: String,
    pub last_seen: DateTime<Utc>,
    pub capacity: u32,
}

pub trait Store: ApiStore + JobStore + RunStore + WorkerStore {}

#[async_trait]
pub trait JobStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>>;

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
        command: &str,
    ) -> Result<bool>; // true if inserted, false if existed
}

#[async_trait]
pub trait RunStore {
    async fn claim_job_runs(&self, worker_id: Uuid, limit: u32) -> Result<Vec<JobRun>>;

    async fn update_job_run_state(
        &self,
        run_id: Uuid,
        new_state: JobRunState,
        exit_code: Option<i32>,
    ) -> Result<()>;
}

#[async_trait]
pub trait WorkerStore {
    async fn heartbeat(&self, worker: &WorkerRecord) -> Result<()>;

    async fn reclaim_dead_workers_jobs(&self, dead_after_secs: u32) -> Result<u64>; // how many jobs requeued
}

#[async_trait]
pub trait ApiStore {
    async fn health_check(&self) -> Result<()>;

    async fn get_job(&self, job_id: Uuid) -> Result<JobSpec>;

    async fn create_job(
        &self,
        name: &str,
        schedule_cron: Option<String>,
        command: String,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
    ) -> Result<JobSpec>;

    async fn list_jobs(&self) -> Result<Vec<JobSpec>>;

    async fn list_recent_runs(
        &self,
        limit: u32,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
    ) -> Result<Vec<JobRun>>;

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()>;

    async fn enable_job(&self, job_id: Uuid) -> Result<()>;

    async fn disable_job(&self, job_id: Uuid) -> Result<()>;

    async fn update_job(
        &self,
        job_id: Uuid,
        name: Option<String>,
        schedule_cron: Option<Option<String>>,
        command: Option<String>,
        max_concurrency: Option<u32>,
        misfire_policy: Option<MisfirePolicy>,
    ) -> Result<JobSpec>;

    async fn delete_job(&self, job_id: Uuid) -> Result<()>;

    async fn create_adhoc_run(
        &self,
        job_id: Uuid,
        command_override: Option<String>,
    ) -> Result<JobRun>;

    async fn cancel_run(&self, run_id: Uuid) -> Result<()>;

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>>;
}
