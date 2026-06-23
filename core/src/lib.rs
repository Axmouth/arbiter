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

/// A database engine a shared connection config targets. The string form
/// (`pgsql`/`mysql`) matches the runner type and selects the backing table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DbEngine {
    PgSql,
    MySql,
}

impl DbEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbEngine::PgSql => "pgsql",
            DbEngine::MySql => "mysql",
        }
    }
}

impl FromStr for DbEngine {
    type Err = ArbiterError;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pgsql" => Ok(DbEngine::PgSql),
            "mysql" => Ok(DbEngine::MySql),
            _ => Err(ArbiterError::InvalidInput(format!("invalid db engine: {s}"))),
        }
    }
}

/// A shared database connection config, referenced by pgsql/mysql runners via id. The
/// password is held as a secret reference (`secret:<name>`) resolved at execution, never
/// as plaintext.
#[derive(Debug, Clone, Serialize, Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SharedDbConfig {
    pub id: Uuid,
    pub engine: DbEngine,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    /// A `secret:<name>` reference for the connection password. Never plaintext.
    pub password_secret: String,
    pub database: String,
    pub tenant_id: Uuid,
}

/// CRUD for shared DB connection configs (pgsql/mysql). Tenant-scoped like jobs/secrets:
/// `scope` = `None` for a system caller (all tenants), `Some(t)` restricts to tenant t.
/// Deletes are soft (runner rows reference configs), so a config in use stays resolvable.
#[async_trait]
pub trait ConfigStore {
    #[allow(clippy::too_many_arguments)]
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
    ) -> Result<SharedDbConfig>;

    async fn get_db_config(&self, id: Uuid, scope: Option<Uuid>) -> Result<Option<SharedDbConfig>>;

    async fn list_db_configs(&self, scope: Option<Uuid>) -> Result<Vec<SharedDbConfig>>;

    #[allow(clippy::too_many_arguments)]
    async fn update_db_config(
        &self,
        id: Uuid,
        name: Option<&str>,
        host: Option<&str>,
        port: Option<u16>,
        username: Option<&str>,
        password_secret: Option<&str>,
        database: Option<&str>,
    ) -> Result<SharedDbConfig>;

    /// Soft-delete a config (set `deleted_at`). A snapshot built from a now-deleted config
    /// fails at claim time; existing references are not broken.
    async fn delete_db_config(&self, id: Uuid) -> Result<()>;
}

pub trait Store:
    ApiStore
    + JobStore
    + RunStore
    + WorkerStore
    + SettingsStore
    + SecretStore
    + TenantStore
    + ConfigStore
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

    /// Resolve when settings may have changed (notification-driven: Postgres
    /// `LISTEN`/`NOTIFY`, in-process for single-node SQLite). Best-effort and may miss
    /// events, so callers pair it with a periodic reload. The default never fires, so a
    /// backend that does not implement it falls back to poll-only refresh.
    async fn await_settings_change(&self) {
        std::future::pending::<()>().await
    }
}

/// Fallback values for runtime settings, taken from the static startup config. Used when
/// a key is absent from the live [`SettingsStore`].
#[derive(Debug, Clone)]
pub struct RuntimeDefaults {
    pub misfire_catchup_secs: u64,
    pub run_retention_secs: u64,
    pub prune_interval_secs: u64,
    /// Scheduler bounded-sleep cap in seconds; `0` = no bound (sleep to the next fire,
    /// relying on change notifications).
    pub scheduler_backstop_secs: u64,
}

/// A typed, auto-refreshing view over the runtime [`SettingsStore`]. Reads are sync and
/// cheap (a cached snapshot); [`RuntimeSettings::spawn_refresh`] keeps the cache fresh by
/// reloading on a change notification, with a periodic poll as a backstop against a missed
/// notification. Accessors fall back to [`RuntimeDefaults`] when a key is unset.
pub struct RuntimeSettings {
    store: std::sync::Arc<dyn SettingsStore + Send + Sync>,
    cache: std::sync::RwLock<HashMap<String, String>>,
    defaults: RuntimeDefaults,
}

impl RuntimeSettings {
    pub fn new(
        store: std::sync::Arc<dyn SettingsStore + Send + Sync>,
        defaults: RuntimeDefaults,
    ) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            store,
            cache: std::sync::RwLock::new(HashMap::new()),
            defaults,
        })
    }

    /// Load the current settings snapshot into the cache.
    pub async fn refresh(&self) -> Result<()> {
        let all = self.store.list_settings().await?;
        let map = all.into_iter().map(|s| (s.key, s.value)).collect();
        *self.cache.write().unwrap_or_else(|p| p.into_inner()) = map;
        Ok(())
    }

    /// Spawn the background refresher: reload on a change notification, or every
    /// `backstop` as insurance against a missed one.
    pub fn spawn_refresh(self: std::sync::Arc<Self>, backstop: std::time::Duration) {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = self.store.await_settings_change() => {}
                    _ = tokio::time::sleep(backstop) => {}
                }
                if let Err(e) = self.refresh().await {
                    tracing::warn!("runtime settings refresh failed: {e}");
                }
            }
        });
    }

    fn raw(&self, key: &str) -> Option<String> {
        self.cache
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
    }

    fn u64_or(&self, key: &str, default: u64) -> u64 {
        self.raw(key).and_then(|v| v.parse().ok()).unwrap_or(default)
    }

    pub fn misfire_catchup_secs(&self) -> u64 {
        self.u64_or(
            "scheduler.misfire_catchup_secs",
            self.defaults.misfire_catchup_secs,
        )
    }

    /// Retention window in seconds (stored as days under `retention.run_retention_days`).
    pub fn run_retention_secs(&self) -> u64 {
        self.raw("retention.run_retention_days")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|days| days * 86_400)
            .unwrap_or(self.defaults.run_retention_secs)
    }

    pub fn prune_interval_secs(&self) -> u64 {
        self.u64_or(
            "retention.prune_interval_secs",
            self.defaults.prune_interval_secs,
        )
    }

    /// Scheduler bounded-sleep cap in seconds (`0` = unbounded; sleep to the next fire).
    pub fn scheduler_backstop_secs(&self) -> u64 {
        self.u64_or(
            "scheduler.backstop_secs",
            self.defaults.scheduler_backstop_secs,
        )
    }
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
    /// Create or replace a secret by (tenant, name); returns its id.
    #[allow(clippy::too_many_arguments)]
    async fn upsert_secret(
        &self,
        tenant_id: Uuid,
        name: &str,
        value_ct: &[u8],
        value_nonce: &[u8],
        aead_algo: &str,
        dek_wrapped: &[u8],
        kek_version: u32,
    ) -> Result<Uuid>;

    /// Look up a secret by name within `tenant` (tenant isolation, SECRETS.md I7).
    async fn get_secret_by_name(
        &self,
        tenant: Uuid,
        name: &str,
    ) -> Result<Option<StoredSecret>>;
    /// `scope` = `None` lists/gets across all tenants (system caller), `Some(t)` restricts.
    async fn get_secret(&self, id: Uuid, scope: Option<Uuid>) -> Result<Option<StoredSecret>>;
    async fn list_secret_names(&self, scope: Option<Uuid>) -> Result<Vec<SecretMeta>>;
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
    /// Resolve a secret by name within the job's tenant. Refuses a secret from another
    /// tenant (fail closed), enforcing tenant isolation (SECRETS.md I7).
    async fn resolve_secret(&self, tenant: Uuid, name: &str) -> Result<String>;
}

/// Encrypt-and-store surface for managing secrets (the write side). Implemented by the
/// secrets layer; the API depends only on this trait, not the crypto stack. Requires a
/// KEK, so it is available only on a node that holds one (see SECRETS.md). Reads (listing
/// metadata, deletion) go through [`SecretStore`] and need no key.
#[async_trait]
pub trait SecretAdmin: Send + Sync {
    /// Create or replace a secret by (tenant, name); returns its id. The value is
    /// encrypted before it touches storage.
    async fn set_secret(&self, tenant: Uuid, name: &str, value: &[u8]) -> Result<Uuid>;
}

#[async_trait]
pub trait JobStore {
    async fn list_enabled_cron_jobs(&self) -> Result<Vec<JobSpec>>;

    async fn insert_job_run_if_missing(
        &self,
        job_id: Uuid,
        scheduled_for: DateTime<Utc>,
    ) -> Result<bool>; // true if inserted, false if existed

    /// The tenant a job belongs to (for resolving its secrets in scope). `None` if the
    /// job does not exist.
    async fn job_tenant(&self, job_id: Uuid) -> Result<Option<Uuid>>;

    /// Resolve when a job's scheduling-relevant state may have changed (create / update /
    /// enable / disable / delete), so the scheduler can replan instead of waiting out its
    /// sleep. Same notify-or-backstop contract as [`SettingsStore::await_settings_change`]:
    /// best-effort, may miss events, paired with the scheduler's bounded sleep. Default
    /// never fires.
    async fn await_jobs_change(&self) {
        std::future::pending::<()>().await
    }
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

    /// `scope` = `None` for a system caller (any tenant), `Some(t)` restricts to tenant t
    /// (returns NotFound if the job is in another tenant).
    async fn get_job(&self, job_id: Uuid, scope: Option<Uuid>) -> Result<JobSpec>;

    #[allow(clippy::too_many_arguments)]
    async fn create_job(
        &self,
        tenant_id: Uuid,
        name: &str,
        schedule_cron: Option<String>,
        runner_cfg: RunnerConfig,
        max_concurrency: u32,
        misfire_policy: MisfirePolicy,
        retry: RetryConfig,
    ) -> Result<JobSpec>;

    async fn list_jobs(&self, scope: Option<Uuid>) -> Result<Vec<JobSpec>>;

    async fn list_recent_runs(
        &self,
        limit: Option<u32>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
        by_job_id: Option<Uuid>,
        by_worker_id: Option<Uuid>,
        scope: Option<Uuid>,
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

    struct FakeSettings(std::sync::Mutex<HashMap<String, String>>);

    #[async_trait]
    impl SettingsStore for FakeSettings {
        async fn get_setting(&self, key: &str) -> Result<Option<String>> {
            Ok(self.0.lock().expect("lock").get(key).cloned())
        }
        async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .expect("lock")
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        async fn list_settings(&self) -> Result<Vec<Setting>> {
            Ok(self
                .0
                .lock()
                .expect("lock")
                .iter()
                .map(|(k, v)| Setting {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect())
        }
    }

    #[tokio::test]
    async fn runtime_settings_reads_live_values_and_falls_back_to_defaults() {
        let store = std::sync::Arc::new(FakeSettings(std::sync::Mutex::new(HashMap::new())));
        let defaults = RuntimeDefaults {
            misfire_catchup_secs: 7,
            run_retention_secs: 30 * 86_400,
            prune_interval_secs: 3600,
            scheduler_backstop_secs: 180,
        };
        let settings = RuntimeSettings::new(store.clone(), defaults);

        // Before any refresh / with nothing stored, accessors return the defaults.
        settings.refresh().await.expect("refresh");
        assert_eq!(settings.misfire_catchup_secs(), 7);
        assert_eq!(settings.run_retention_secs(), 30 * 86_400);
        assert_eq!(settings.prune_interval_secs(), 3600);

        // A stored override is reflected after a refresh; retention is stored in days.
        store
            .set_setting("scheduler.misfire_catchup_secs", "120")
            .await
            .unwrap();
        store
            .set_setting("retention.run_retention_days", "2")
            .await
            .unwrap();
        settings.refresh().await.expect("refresh");
        assert_eq!(settings.misfire_catchup_secs(), 120);
        assert_eq!(settings.run_retention_secs(), 2 * 86_400);
        // Unset key still falls back.
        assert_eq!(settings.prune_interval_secs(), 3600);

        // A garbage value falls back to the default rather than panicking.
        store
            .set_setting("scheduler.misfire_catchup_secs", "not-a-number")
            .await
            .unwrap();
        settings.refresh().await.expect("refresh");
        assert_eq!(settings.misfire_catchup_secs(), 7);
    }

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
