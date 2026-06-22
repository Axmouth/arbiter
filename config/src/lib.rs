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
}

impl Default for SchedulerSettings {
    fn default() -> Self {
        Self {
            misfire_catchup_secs: 0,
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
    pub retention: RetentionConfig,
    #[serde(default)]
    pub scheduler: SchedulerSettings,
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

        // Environment overrides
        builder = builder.add_source(Environment::with_prefix("ARBITER").separator("_"));

        builder
            .build()
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .try_deserialize()
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))
    }
}
