use config::{Config, Environment, File};
use arbiter_core::{ArbiterError, Result};
use serde::Deserialize;
use std::path::PathBuf;

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

#[derive(Debug, Deserialize, Clone)]
pub struct WebConfig {
    pub database: DbConfig,
    pub admin: AdminConfig,
    pub api: ApiConfig,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            database: DbConfig {
                url: "postgres://arbiter:arbiter@localhost:5432/arbiter".into(),
            },
            api: ApiConfig {
                jwt_secret: "devsecret".into(),
                port: 8080,
            },
            admin: AdminConfig {
                username: "admin".into(),
                password: "admin".into(),
            },
        }
    }
}

impl WebConfig {
    pub fn try_load() -> Result<Self> {
        let mut builder = Config::builder()
            .set_default("database.url", WebConfig::default().database.url)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("api.jwt_secret", WebConfig::default().api.jwt_secret)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("api.port", WebConfig::default().api.port)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("admin.username", WebConfig::default().admin.username)
            .map_err(|e| ArbiterError::ValidationError(e.to_string()))?
            .set_default("admin.password", WebConfig::default().admin.password)
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

#[derive(Debug, Deserialize, Clone)]
pub struct NodeConfig {
    pub database: DbConfig,
}

impl NodeConfig {
    pub fn try_load() -> Result<Self> {
        let mut builder = Config::builder()
            .set_default("database.url", WebConfig::default().database.url)
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
