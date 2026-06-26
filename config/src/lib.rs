use config::{Config, Environment, File};
use arbiter_core::{ArbiterError, Result};
use serde::Deserialize;
use std::path::PathBuf;

/// The default database url, used when nothing overrides it (dev/single-node).
pub const DEFAULT_DATABASE_URL: &str = "postgres://arbiter:arbiter@localhost:5432/arbiter";

#[derive(Debug, Deserialize, Clone)]
pub struct AdminConfig {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DbConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ApiConfig {
    pub jwt_secret: String,
    pub port: u16,
}

/// Which roles this node runs. A node is always a cluster member with its own
/// identity; the roles only decide which loops/servers it spawns. A deployment
/// composes itself by toggling these (e.g. api-only control plane, worker-only
/// fleet, or all-in-one for single-node). Default: all on.
#[derive(Debug, Deserialize, Clone)]
pub struct RolesConfig {
    pub api: bool,
    pub scheduler: bool,
    pub worker: bool,
}

impl Default for RolesConfig {
    fn default() -> Self {
        Self {
            api: true,
            scheduler: true,
            worker: true,
        }
    }
}

/// Node-local runtime knobs (where identity is persisted, single-vs-multi identity).
/// These used to be read straight from the environment in `arbiter-node`; they live
/// here so the config crate is the only place that touches the environment.
#[derive(Debug, Deserialize, Clone)]
pub struct NodeSettings {
    /// Directory the node persists its identity file in (default `/data`).
    pub data_dir: String,
    /// Path to the node's crypto identity keyring file.
    pub identity_path: String,
    /// Allow several node processes on one host to take distinct identities.
    pub allow_multi_id: bool,
}

impl Default for NodeSettings {
    fn default() -> Self {
        Self {
            data_dir: "/data".into(),
            identity_path: "node_identity.json".into(),
            allow_multi_id: false,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetentionConfig {
    /// Keep terminal runs for this many days; the leader prunes older ones.
    /// `0` disables retention (keep runs forever).
    pub run_retention_days: u32,
    /// How often the leader runs a retention prune.
    pub prune_interval_secs: u64,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            run_retention_days: 0,
            prune_interval_secs: 3600,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct SchedulerSettings {
    /// Max look-back for misfire catch-up; `0` disables backfill.
    pub misfire_catchup_secs: u64,
    /// The leader's bounded-sleep cap in seconds: it sleeps until the next fire but at
    /// most this long, waking on a job change in between. `0` = no bound (sleep to the
    /// next fire, relying on change notifications). Default 180 (3 min).
    pub backstop_secs: u64,
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            misfire_catchup_secs: 0,
            backstop_secs: 180,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkerSettings {
    /// How long a worker sleeps when nothing is due before re-polling. The runs
    /// notification handles promptness, so this only bounds idle polling. `0` = no bound
    /// (rely on the notification). Default 300 (5 min).
    pub claim_backstop_secs: u64,
    /// Per-run cap on captured output in bytes (stdout + stderr). Past it the log is
    /// truncated with a marker so a runaway job cannot fill storage. `0` = no cap.
    /// Default 10 MiB.
    pub max_log_bytes: u64,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            claim_backstop_secs: 300,
            max_log_bytes: 10 * 1024 * 1024,
        }
    }
}

/// Config for a node, the single binary that can run any subset of roles.
#[derive(Debug, Deserialize, Clone)]
pub struct NodeConfig {
    pub database: DbConfig,
    pub admin: AdminConfig,
    pub api: ApiConfig,
    #[serde(default)]
    pub roles: RolesConfig,
    #[serde(default)]
    pub node: NodeSettings,
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub scheduler: SchedulerSettings,
    #[serde(default)]
    pub worker: WorkerSettings,
}

impl NodeConfig {
    pub fn try_load() -> Result<Self> {
        let mut builder = Config::builder()
            .set_default("database.url", DEFAULT_DATABASE_URL)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("admin.username", "admin")
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("admin.password", "admin")
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("api.jwt_secret", "devsecret")
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("api.port", 8080)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("roles.api", RolesConfig::default().api)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("roles.scheduler", RolesConfig::default().scheduler)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("roles.worker", RolesConfig::default().worker)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "retention.run_retention_days",
                RetentionConfig::default().run_retention_days as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "retention.prune_interval_secs",
                RetentionConfig::default().prune_interval_secs as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "scheduler.misfire_catchup_secs",
                SchedulerSettings::default().misfire_catchup_secs as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "scheduler.backstop_secs",
                SchedulerSettings::default().backstop_secs as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "worker.claim_backstop_secs",
                WorkerSettings::default().claim_backstop_secs as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default(
                "worker.max_log_bytes",
                WorkerSettings::default().max_log_bytes as i64,
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("node.data_dir", NodeSettings::default().data_dir)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("node.identity_path", NodeSettings::default().identity_path)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("node.allow_multi_id", NodeSettings::default().allow_multi_id)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?;

        // Search paths
        let mut search_paths: Vec<String> = vec![
            "./config/arbiter.toml".into(),
            "./arbiter.toml".into(),
            "/etc/arbiter/arbiter.toml".into(),
        ];
        if let Some(home_dir) = dirs::home_dir() {
            search_paths.push(format!("{}/.config/arbiter/arbiter.toml", home_dir.display()));
            search_paths.push(format!("{}/arbiter/arbiter.toml", home_dir.display()));
        }

        for path in &search_paths {
            let p = PathBuf::from(path);
            if p.exists() {
                builder = builder.add_source(File::from(p.as_path()));
            }
        }

        // Environment overrides. The generic source handles nested keys via `_`, which
        // cannot target a field whose own name contains `_` (e.g. node.data_dir). Those
        // few are mapped explicitly below so the environment is still read only here.
        builder = builder.add_source(Environment::with_prefix("ARBITER").separator("_"));

        builder = builder
            .set_override_option("node.data_dir", std::env::var("ARBITER_DATA_DIR").ok())
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_override_option(
                "node.identity_path",
                std::env::var("ARBITER_NODE_IDENTITY").ok(),
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_override_option(
                "node.allow_multi_id",
                std::env::var("ARBITER_ALLOW_MULTI_ID")
                    .ok()
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true")),
            )
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?;

        builder
            .build()
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .try_deserialize()
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))
    }
}
