use std::{collections::HashMap, fmt, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utoipa::ToSchema;
use uuid::Uuid;

// TODO: Secret type that holds id only, must be resolved at last moment from storage and secret handling.

#[derive(Debug, Clone)]
pub enum DromioError {
    Conflict(String),
    DatabaseError(String),
    ExecutionError(String),
    InvalidInput(String),
    NotFound(String),
    ValidationError(String),
}

impl From<sqlx::Error> for DromioError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(dberr) = &err
            && dberr.is_unique_violation()
        {
            return DromioError::Conflict("Object".to_string());
        }
        DromioError::DatabaseError(err.to_string())
    }
}

impl From<std::num::ParseIntError> for DromioError {
    fn from(err: std::num::ParseIntError) -> Self {
        DromioError::InvalidInput(err.to_string())
    }
}

impl fmt::Display for DromioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DromioError::Conflict(msg) => write!(f, "Conlfict error: {} already exists", msg),
            DromioError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            DromioError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            DromioError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            DromioError::NotFound(msg) => write!(f, "Not found: {}", msg),
            DromioError::ValidationError(msg) => write!(f, "Validation Error: {}", msg),
        }
    }
}

impl std::error::Error for DromioError {}

pub type Result<T> = std::result::Result<T, DromioError>;

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobSpec {
    pub id: Uuid,
    pub name: String,
    pub schedule_cron: Option<String>, // cron syntax, TODO: use a proper type instead of unsafe String
    pub enabled: bool,
    pub runner_cfg: RunnerConfig,
    pub max_concurrency: u32,
    pub misfire_policy: MisfirePolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
#[ts(export)]
pub enum RunnerConfig {
    #[serde(rename_all = "camelCase")]
    Shell {
        command: String,
        working_dir: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Http {
        method: String,
        url: String,
        headers: Option<HashMap<String, String>>,
        body: Option<String>,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    PgSql {
        config_id: Uuid,
        query: String,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    MySql {
        config_id: Uuid,
        query: String,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Python {
        module: String,
        class_name: String,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Node {
        module: String,
        function_name: String,
        timeout_sec: Option<u32>,
    },
}

impl RunnerConfig {
    pub fn type_of_str(&self) -> &str {
        match self {
            RunnerConfig::Shell { .. } => "shell",
            RunnerConfig::Http { .. } => "http",
            RunnerConfig::PgSql { .. } => "pgsql",
            RunnerConfig::MySql { .. } => "mysql",
            RunnerConfig::Python { .. } => "python",
            RunnerConfig::Node { .. } => "node",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SharedConfig {
    name: String,
    meta: SharedConfigMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
#[ts(export)]
pub enum SharedConfigMeta {
    #[serde(rename_all = "camelCase")]
    Shell { env: HashMap<String, String> },
    #[serde(rename_all = "camelCase")]
    PgSql {
        host: String,
        port: u16,
        username: String,
        password_secret: String,
        database: String,
    },
    #[serde(rename_all = "camelCase")]
    MySql {
        host: String,
        port: u16,
        username: String,
        password_secret: String,
        database: String,
    },
    // ... same idea for others
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExecutableConfigSnapshot {
    pub name: Option<String>,
    pub job_name: String,
    pub meta: ExecutableConfigSnapshotMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase", tag = "type")]
#[ts(export)]
pub enum ExecutableConfigSnapshotMeta {
    #[serde(rename_all = "camelCase")]
    Shell {
        command: String,
        working_dir: Option<String>,
        env: HashMap<String, String>,
    },
    #[serde(rename_all = "camelCase")]
    PgSql {
        host: String,
        port: u16,
        username: String,
        password_secret: String,
        database: String,
        query: String,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    MySql {
        host: String,
        port: u16,
        username: String,
        password_secret: String,
        database: String,
        query: String,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Http {
        // TODO: stricter types
        method: String,
        url: String,
        headers: HashMap<String, String>,
        body: Option<String>,
        timeout_sec: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Node {
        module: String,
        function_name: String,
        timeout_sec: Option<u32>,
        env: HashMap<String, String>,
    },
    #[serde(rename_all = "camelCase")]
    Python {
        module: String,
        class_name: String,
        timeout_sec: Option<u32>,
        env: HashMap<String, String>,
    },
}

impl ExecutableConfigSnapshotMeta {
    pub fn type_of_str(&self) -> &str {
        match self {
            ExecutableConfigSnapshotMeta::Shell { .. } => "shell",
            ExecutableConfigSnapshotMeta::Http { .. } => "http",
            ExecutableConfigSnapshotMeta::PgSql { .. } => "pgsql",
            ExecutableConfigSnapshotMeta::MySql { .. } => "mysql",
            ExecutableConfigSnapshotMeta::Python { .. } => "python",
            ExecutableConfigSnapshotMeta::Node { .. } => "node",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobRunState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl fmt::Display for JobRunState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            JobRunState::Queued => "queued",
            JobRunState::Running => "running",
            JobRunState::Succeeded => "succeeded",
            JobRunState::Failed => "failed",
            JobRunState::Cancelled => "cancelled",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for JobRunState {
    type Err = DromioError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "queued" => Ok(JobRunState::Queued),
            "running" => Ok(JobRunState::Running),
            "succeeded" => Ok(JobRunState::Succeeded),
            "failed" => Ok(JobRunState::Failed),
            "cancelled" => Ok(JobRunState::Cancelled),
            _ => Err(DromioError::InvalidInput(format!(
                "invalid job run state: {}",
                s
            ))),
        }
    }
}

pub struct SchedulerConfig {
    pub tick_interval_ms: u64,
}

pub struct WorkerConfig {
    pub worker_id: Uuid,
    pub display_name: String,
    pub capacity: u32,
    pub hostname: String,
    pub tick_interval_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub dead_after_secs: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
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

// TODO: Consider converting to enum for better state alignment?
#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
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
    pub snapshot: Option<ExecutableConfigSnapshot>,
    pub output: Option<String>,
    pub error_output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkerRecord {
    pub id: Uuid,
    pub display_name: String,
    pub hostname: String,
    pub last_seen: DateTime<Utc>,
    pub capacity: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UserRole {
    Admin,
    Tenant,
    Operator,
    Viewer,
}

impl std::str::FromStr for UserRole {
    type Err = DromioError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(UserRole::Admin),
            "tenant" => Ok(UserRole::Tenant),
            "operator" => Ok(UserRole::Operator),
            "viewer" => Ok(UserRole::Viewer),
            _ => Err(DromioError::InvalidInput(format!("invalid role: {}", s))),
        }
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
            UserRole::Tenant => write!(f, "tenant"),
            UserRole::Operator => write!(f, "operator"),
            UserRole::Viewer => write!(f, "viewer"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    #[serde(skip)]
    pub password_hash: String,
    pub role: UserRole,
    pub created_at: DateTime<Utc>,
}

pub trait Store: ApiStore + JobStore + RunStore + WorkerStore {}

#[async_trait]
pub trait JobStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>>;

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
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
        output: Option<String>,
        error_output: Option<String>,
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
        runner_cfg: RunnerConfig,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
    ) -> Result<JobSpec>;

    async fn list_jobs(&self) -> Result<Vec<JobSpec>>;

    async fn list_recent_runs(
        &self,
        limit: Option<u32>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
        by_worker_id: Option<Uuid>,
    ) -> Result<Vec<JobRun>>;

    async fn set_job_enabled(&self, job_id: Uuid, enabled: bool) -> Result<()>;

    async fn enable_job(&self, job_id: Uuid) -> Result<()>;

    async fn disable_job(&self, job_id: Uuid) -> Result<()>;

    async fn update_job(
        &self,
        job_id: Uuid,
        name: Option<String>,
        schedule_cron: Option<Option<String>>,
        runner_cfg: Option<RunnerConfig>,
        max_concurrency: Option<u32>,
        misfire_policy: Option<MisfirePolicy>,
    ) -> Result<JobSpec>;

    async fn delete_job(&self, job_id: Uuid) -> Result<()>;

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun>;

    async fn cancel_run(&self, run_id: Uuid) -> Result<()>;

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>>;

    async fn get_user_by_username(&self, username: &str) -> Result<User>;
    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User>;
    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
    ) -> Result<User>;
    async fn list_users(&self) -> Result<Vec<User>>;
    async fn delete_user(&self, user_id: Uuid) -> Result<()>;
    async fn update_password(&self, user_id: Uuid, password_hash: &str) -> Result<()>;
    async fn update_user(
        &self,
        user_id: Uuid,
        username: Option<&str>,
        password_hash: Option<&str>,
        role: Option<UserRole>,
    ) -> Result<User>;
    async fn count_users(&self) -> Result<u32>;
}
