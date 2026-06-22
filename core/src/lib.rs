use std::{collections::HashMap, fmt, str::FromStr};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use utoipa::ToSchema;
use uuid::Uuid;

// TODO: Secret type that holds id only, must be resolved at last moment from storage and secret handling.

#[derive(Debug, Clone)]
pub enum ArbiterError {
    Conflict(String),
    DatabaseError(String),
    ExecutionError(String),
    InvalidInput(String),
    NotFound(String),
    ValidationError(String),
}

impl From<sqlx::Error> for ArbiterError {
    fn from(err: sqlx::Error) -> Self {
        if let sqlx::Error::Database(dberr) = &err
            && dberr.is_unique_violation()
        {
            return ArbiterError::Conflict("Object".to_string());
        }
        ArbiterError::DatabaseError(err.to_string())
    }
}

impl From<std::num::ParseIntError> for ArbiterError {
    fn from(err: std::num::ParseIntError) -> Self {
        ArbiterError::InvalidInput(err.to_string())
    }
}

impl fmt::Display for ArbiterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArbiterError::Conflict(msg) => write!(f, "Conlfict error: {} already exists", msg),
            ArbiterError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ArbiterError::ExecutionError(msg) => write!(f, "Execution error: {}", msg),
            ArbiterError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            ArbiterError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ArbiterError::ValidationError(msg) => write!(f, "Validation Error: {}", msg),
        }
    }
}

impl std::error::Error for ArbiterError {}

pub type Result<T> = std::result::Result<T, ArbiterError>;

/// Sleep for a duration plus a random jitter up to `jitter`% of the duration.
pub async fn snooze(duration: std::time::Duration, jitter: u64) {
    let jitter_us = rand::random::<u64>() % ((duration.as_micros() as u64 / 100) * jitter);
    let duration = duration + std::time::Duration::from_micros(jitter_us);
    tokio::time::sleep(duration).await;
}

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
    /// Retry config. `max_attempts = 1` means no retry. A `retryable` run is requeued
    /// with a backoff (jittered) until attempts are exhausted, then it fails.
    pub max_attempts: u32,
    pub backoff_strategy: BackoffStrategy,
    pub backoff_base_secs: u32,
    pub backoff_cap_secs: u32,
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

/// Per-job retry configuration. `Default` = no retry (`max_attempts = 1`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub backoff_strategy: BackoffStrategy,
    pub backoff_base_secs: u32,
    pub backoff_cap_secs: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff_strategy: BackoffStrategy::Exponential,
            backoff_base_secs: 30,
            backoff_cap_secs: 3600,
        }
    }
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
    type Err = ArbiterError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "queued" => Ok(JobRunState::Queued),
            "running" => Ok(JobRunState::Running),
            "succeeded" => Ok(JobRunState::Succeeded),
            "failed" => Ok(JobRunState::Failed),
            "cancelled" => Ok(JobRunState::Cancelled),
            _ => Err(ArbiterError::InvalidInput(format!(
                "invalid job run state: {}",
                s
            ))),
        }
    }
}

pub struct SchedulerConfig {
    pub tick_interval_ms: u64,
    /// Max look-back window for misfire catch-up. `0` disables backfill (only future
    /// fires materialize); per-job `MisfirePolicy` refines behavior within the window.
    pub misfire_catchup_secs: u64,
}

pub struct WorkerConfig {
    pub worker_id: Uuid,
    pub display_name: String,
    pub capacity: u32,
    pub hostname: String,
    pub tick_interval_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub dead_after_secs: u32,
    pub restart_count: u32,
    pub version: String,
    /// Retain terminal runs for this many seconds; the leader prunes older ones.
    /// `0` disables retention (keep runs forever).
    pub run_retention_secs: u64,
    /// How often the leader runs a retention prune.
    pub prune_interval_secs: u64,
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
    type Err = ArbiterError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "skip" => Ok(MisfirePolicy::Skip),
            "run_immediately" => Ok(MisfirePolicy::RunImmediately),
            "coalesce" => Ok(MisfirePolicy::Coalesce),
            "run_all" => Ok(MisfirePolicy::RunAll),
            _ if s.starts_with("run_if_late_within(") && s.ends_with(")") => {
                let dur_str = &s["run_if_late_within(".len()..s.len() - 1];
                let dur = dur_str.parse::<u32>().map_err(|_| {
                    ArbiterError::InvalidInput(format!("Invalid duration: {}", dur_str))
                })?;
                Ok(MisfirePolicy::RunIfLateWithin(Duration::seconds(
                    dur as i64,
                )))
            }
            _ => Err(ArbiterError::InvalidInput(format!(
                "Unknown misfire policy: {}",
                s
            ))),
        }
    }
}

/// The classification a runner reports for a finished attempt, distinct from the
/// process `exit_code` and the lifecycle `state`. `Retryable` asks the scheduler to
/// requeue the run (up to the job's `max_attempts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ResultStatus {
    Success,
    Failed,
    Retryable,
}

impl fmt::Display for ResultStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ResultStatus::Success => "success",
            ResultStatus::Failed => "failed",
            ResultStatus::Retryable => "retryable",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for ResultStatus {
    type Err = ArbiterError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "success" => Ok(ResultStatus::Success),
            "failed" => Ok(ResultStatus::Failed),
            "retryable" => Ok(ResultStatus::Retryable),
            _ => Err(ArbiterError::InvalidInput(format!(
                "invalid result status: {s}"
            ))),
        }
    }
}

/// Per-job retry backoff shape. Jitter is always applied on top (mandatory), so a
/// distributed fleet does not re-hit a recovering dependency in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BackoffStrategy {
    Fixed,
    Exponential,
    Fibonacci,
}

impl fmt::Display for BackoffStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BackoffStrategy::Fixed => "fixed",
            BackoffStrategy::Exponential => "exponential",
            BackoffStrategy::Fibonacci => "fibonacci",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for BackoffStrategy {
    type Err = ArbiterError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "fixed" => Ok(BackoffStrategy::Fixed),
            "exponential" => Ok(BackoffStrategy::Exponential),
            "fibonacci" => Ok(BackoffStrategy::Fibonacci),
            _ => Err(ArbiterError::InvalidInput(format!(
                "invalid backoff strategy: {s}"
            ))),
        }
    }
}

fn fib(n: u32) -> u64 {
    let (mut a, mut b) = (1u64, 1u64);
    for _ in 1..n.max(1) {
        let c = a.saturating_add(b);
        a = b;
        b = c;
    }
    a
}

/// Raw (un-jittered) backoff in seconds for the attempt that just failed (1-based),
/// capped at `cap_secs`. Pure, so it is unit-testable; `next_retry_delay` jitters it.
pub fn retry_backoff_secs(
    strategy: BackoffStrategy,
    base_secs: u32,
    cap_secs: u32,
    attempt: u32,
) -> u64 {
    let n = attempt.max(1);
    let raw = match strategy {
        BackoffStrategy::Fixed => base_secs as u64,
        BackoffStrategy::Exponential => {
            (base_secs as u64).saturating_mul(1u64.checked_shl(n - 1).unwrap_or(u64::MAX))
        }
        BackoffStrategy::Fibonacci => (base_secs as u64).saturating_mul(fib(n)),
    };
    raw.min(cap_secs as u64)
}

/// Delay before the next attempt: capped backoff with mandatory full jitter
/// (uniform in `[0, raw]`).
pub fn next_retry_delay(
    strategy: BackoffStrategy,
    base_secs: u32,
    cap_secs: u32,
    attempt: u32,
) -> Duration {
    let raw = retry_backoff_secs(strategy, base_secs, cap_secs, attempt);
    let jittered = if raw == 0 {
        0
    } else {
        rand::random::<u64>() % (raw + 1)
    };
    Duration::seconds(jittered as i64)
}

/// The outcome of one execution attempt, reported by the worker. Streams
/// (`stdout`/`stderr`) are plain text; the typed payloads (`result`/`error`) carry a
/// media type so any format (json/text/html/...) is self-describing.
#[derive(Debug, Clone, Default)]
pub struct RunOutcome {
    pub status: Option<ResultStatus>,
    pub exit_code: Option<i32>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub result: Option<String>,
    pub result_media_type: Option<String>,
    pub error: Option<String>,
    pub error_media_type: Option<String>,
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
    pub attempt: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub snapshot: Option<ExecutableConfigSnapshot>,
    pub result_status: Option<ResultStatus>,
    /// Captured stdout stream (plain text).
    pub stdout: Option<String>,
    /// Captured stderr stream (plain text).
    pub stderr: Option<String>,
    /// The typed result payload (return value / response body) and its media type.
    pub result: Option<String>,
    pub result_media_type: Option<String>,
    /// The typed error payload (structured failure) and its media type.
    pub error: Option<String>,
    pub error_media_type: Option<String>,
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
    pub restart_count: u32,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum UserRole {
    Admin,
    Operator,
    Viewer,
}

impl std::str::FromStr for UserRole {
    type Err = ArbiterError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "admin" => Ok(UserRole::Admin),
            "operator" => Ok(UserRole::Operator),
            "viewer" => Ok(UserRole::Viewer),
            _ => Err(ArbiterError::InvalidInput(format!("invalid role: {}", s))),
        }
    }
}

impl fmt::Display for UserRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UserRole::Admin => write!(f, "admin"),
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
    /// Scope: `None` = system-wide (all tenants), `Some(id)` = that tenant only.
    pub tenant_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// The well-known tenant that owns rows created before/without an explicit tenant.
pub const DEFAULT_TENANT_ID: Uuid = Uuid::from_u128(0x0000_0000_0000_0000_0000_0000_0000_0001);

/// A tenant: the unit of data isolation. Jobs, secrets, and configs belong to one.
#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Tenant {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

/// Tenant CRUD. Scoping of other resources by tenant is enforced in their own queries.
#[async_trait]
pub trait TenantStore {
    async fn create_tenant(&self, name: &str) -> Result<Tenant>;
    async fn get_tenant(&self, id: Uuid) -> Result<Option<Tenant>>;
    async fn list_tenants(&self) -> Result<Vec<Tenant>>;
}

pub trait Store:
    ApiStore + JobStore + RunStore + WorkerStore + SettingsStore + SecretStore + TenantStore
{
}

/// A runtime, admin-settable configuration entry. Values are opaque strings;
/// consumers parse them (with a static-config default fallback) at use-time.
#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Setting {
    pub key: String,
    pub value: String,
}

/// Cluster-wide runtime settings, stored in the shared backend and read live.
#[async_trait]
pub trait SettingsStore {
    async fn get_setting(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str) -> Result<()>;
    async fn list_settings(&self) -> Result<Vec<Setting>>;
}

/// An encrypted secret as stored: ciphertext + wrapped DEK + the KEK version that
/// wrapped it. The store treats all of these as opaque bytes (no crypto knowledge).
#[derive(Debug, Clone)]
pub struct StoredSecret {
    pub id: Uuid,
    pub name: String,
    pub value_ct: Vec<u8>,
    pub value_nonce: Vec<u8>,
    pub aead_algo: String,
    pub dek_wrapped: Vec<u8>,
    pub kek_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Secret metadata without any ciphertext (listing never exposes secret bytes).
#[derive(Debug, Clone)]
pub struct SecretMeta {
    pub id: Uuid,
    pub name: String,
    pub kek_version: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A KEK keyring version and its lifecycle state (e.g. pending/active/retired).
#[derive(Debug, Clone)]
pub struct StoredKekVersion {
    pub version: u32,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub retired_at: Option<DateTime<Utc>>,
}

/// A KEK version sealed to one node's public key, with the node's ack once loaded.
#[derive(Debug, Clone)]
pub struct StoredKekShare {
    pub version: u32,
    pub node_id: Uuid,
    pub wrapped_kek: Vec<u8>,
    pub acked_at: Option<DateTime<Utc>>,
}

/// A node's registered public key (one per node key version) and approval status.
#[derive(Debug, Clone)]
pub struct StoredNodeKey {
    pub node_id: Uuid,
    pub key_version: u32,
    pub public_key: Vec<u8>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub approved_at: Option<DateTime<Utc>>,
}

/// Persistence for the secrets subsystem. Stores only ciphertext, sealed key blobs,
/// and public keys plus metadata, never plaintext values or unwrapped keys. The crypto
/// envelope (DEK/KEK semantics) lives in `arbiter-secrets` on top of this.
#[async_trait]
pub trait SecretStore {
    /// Create or replace a secret by name; returns its id.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_secret(
        &self,
        name: &str,
        value_ct: &[u8],
        value_nonce: &[u8],
        aead_algo: &str,
        dek_wrapped: &[u8],
        kek_version: u32,
    ) -> Result<Uuid>;

    async fn get_secret_by_name(&self, name: &str) -> Result<Option<StoredSecret>>;
    async fn get_secret(&self, id: Uuid) -> Result<Option<StoredSecret>>;
    async fn list_secret_names(&self) -> Result<Vec<SecretMeta>>;
    async fn delete_secret(&self, id: Uuid) -> Result<()>;

    async fn insert_kek_version(&self, version: u32, state: &str) -> Result<()>;
    async fn list_kek_versions(&self) -> Result<Vec<StoredKekVersion>>;

    async fn put_kek_share(&self, version: u32, node_id: Uuid, wrapped_kek: &[u8]) -> Result<()>;
    async fn get_kek_share(&self, version: u32, node_id: Uuid) -> Result<Option<StoredKekShare>>;

    async fn upsert_node_key(
        &self,
        node_id: Uuid,
        key_version: u32,
        public_key: &[u8],
        status: &str,
    ) -> Result<()>;
    async fn list_node_keys(&self) -> Result<Vec<StoredNodeKey>>;
}

/// Resolves a secret reference to its plaintext at execution time. Implemented by the
/// secrets layer; the worker depends only on this trait, not the crypto stack.
#[async_trait]
pub trait SecretResolver: Send + Sync {
    async fn resolve_secret(&self, name: &str) -> Result<String>;
}

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

    /// Finalize a run to a terminal state (succeeded/failed/cancelled), recording the
    /// attempt's outcome (streams, typed result/error, status, exit code).
    async fn finalize_run(
        &self,
        run_id: Uuid,
        new_state: JobRunState,
        outcome: RunOutcome,
    ) -> Result<()>;

    /// Requeue a retryable run for another attempt: record the failed attempt's
    /// outcome, then set state back to queued with the given attempt number and
    /// future `scheduled_for`, clearing the worker/started_at.
    async fn reschedule_for_retry(
        &self,
        run_id: Uuid,
        attempt: u32,
        scheduled_for: DateTime<Utc>,
        outcome: RunOutcome,
    ) -> Result<()>;

    /// Delete terminal runs (succeeded/failed/cancelled) scheduled before
    /// `older_than`. Active runs (queued/running) are never pruned regardless of
    /// age. Returns the number of runs deleted.
    async fn prune_runs(&self, older_than: DateTime<Utc>) -> Result<u64>;
}

#[async_trait]
pub trait WorkerStore {
    async fn heartbeat(&self, worker: &WorkerRecord) -> Result<()>;

    async fn lookup_by_id(&self, id: Uuid) -> Result<Option<(String, u32)>>;

    async fn incr_restart_count(&self, id: Uuid, version: &str) -> Result<u32>;

    async fn insert_worker(
        &self,
        id: Uuid,
        display_name: &str,
        hostname: &str,
        version: &str,
        restart_count: u32,
    ) -> Result<()>;

    async fn reclaim_dead_workers_jobs(&self, dead_after_secs: u32) -> Result<u64>; // how many jobs requeued

    async fn am_i_leader(&self) -> Result<bool>;
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
        retry: RetryConfig,
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

    #[allow(clippy::too_many_arguments)]
    async fn update_job(
        &self,
        job_id: Uuid,
        name: Option<String>,
        schedule_cron: Option<Option<String>>,
        runner_cfg: Option<RunnerConfig>,
        max_concurrency: Option<u32>,
        misfire_policy: Option<MisfirePolicy>,
        retry: Option<RetryConfig>,
    ) -> Result<JobSpec>;

    async fn delete_job(&self, job_id: Uuid) -> Result<()>;

    async fn create_adhoc_run(&self, job_id: Uuid) -> Result<JobRun>;

    async fn cancel_run(&self, run_id: Uuid) -> Result<()>;

    /// Replace a job's environment variables (injected into subprocess runners).
    /// Replace-all semantics: the supplied map becomes the job's complete env.
    ///
    /// Values are plaintext today. Once secrets land, a value may instead be a
    /// secret reference (convention: a `secret:<id>` prefix) resolved by the worker
    /// at execution time via the SecretStore -- the snapshot stores the reference,
    /// never the plaintext. See FOLLOWUPS §13.
    async fn set_job_env(&self, job_id: Uuid, env: HashMap<String, String>) -> Result<()>;

    async fn get_job_env(&self, job_id: Uuid) -> Result<HashMap<String, String>>;

    async fn list_workers(&self) -> Result<Vec<WorkerRecord>>;

    async fn get_user_by_username(&self, username: &str) -> Result<User>;
    async fn get_user_by_id(&self, user_id: Uuid) -> Result<User>;
    async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: UserRole,
        tenant_id: Option<Uuid>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_grows_and_caps() {
        let s = BackoffStrategy::Exponential;
        assert_eq!(retry_backoff_secs(s, 30, 3600, 1), 30);
        assert_eq!(retry_backoff_secs(s, 30, 3600, 2), 60);
        assert_eq!(retry_backoff_secs(s, 30, 3600, 3), 120);
        assert_eq!(retry_backoff_secs(s, 30, 3600, 4), 240);
        // caps at cap_secs
        assert_eq!(retry_backoff_secs(s, 30, 100, 4), 100);
        // huge attempt does not overflow
        assert_eq!(retry_backoff_secs(s, 30, 3600, 1000), 3600);
    }

    #[test]
    fn fibonacci_backoff_follows_sequence() {
        let s = BackoffStrategy::Fibonacci;
        // base * fib(n): fib = 1,1,2,3,5,8,...
        assert_eq!(retry_backoff_secs(s, 10, 100000, 1), 10);
        assert_eq!(retry_backoff_secs(s, 10, 100000, 2), 10);
        assert_eq!(retry_backoff_secs(s, 10, 100000, 3), 20);
        assert_eq!(retry_backoff_secs(s, 10, 100000, 4), 30);
        assert_eq!(retry_backoff_secs(s, 10, 100000, 5), 50);
        assert_eq!(retry_backoff_secs(s, 10, 100000, 6), 80);
    }

    #[test]
    fn fixed_backoff_is_constant() {
        let s = BackoffStrategy::Fixed;
        assert_eq!(retry_backoff_secs(s, 45, 3600, 1), 45);
        assert_eq!(retry_backoff_secs(s, 45, 3600, 9), 45);
    }

    #[test]
    fn jitter_stays_within_capped_backoff() {
        // full jitter: delay in [0, raw]; never exceeds the capped backoff.
        for attempt in 1..=6 {
            for _ in 0..100 {
                let d = next_retry_delay(BackoffStrategy::Exponential, 30, 600, attempt);
                let raw = retry_backoff_secs(BackoffStrategy::Exponential, 30, 600, attempt) as i64;
                let secs = d.num_seconds();
                assert!(secs >= 0 && secs <= raw, "delay {secs} out of [0,{raw}]");
            }
        }
    }
}
