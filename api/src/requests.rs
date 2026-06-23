use arbiter_core::{MisfirePolicy, RetryConfig, RunnerConfig};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use ts_rs::TS;
use utoipa::ToSchema;

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetSettingRequest {
    pub value: String,
}

/// Create a tenant (system admin only).
#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateTenantRequest {
    pub name: String,
}

/// Create or replace a secret. The value is write-only: it is encrypted on the way in
/// and never returned by any endpoint.
#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateSecretRequest {
    pub name: String,
    pub value: String,
}

/// Replace a job's environment variables (replace-all).
#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetJobEnvRequest {
    pub env: HashMap<String, String>,
}

fn some_option<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateJobRequest {
    pub name: String,
    pub schedule_cron: Option<String>,
    pub runner_config: RunnerConfig,
    pub max_concurrency: Option<u32>,
    pub misfire_policy: Option<MisfirePolicy>,
    /// Optional retry policy (defaults to no retry).
    pub retry: Option<RetryConfig>,
    /// Optional initial environment variables for the job's runner.
    pub env: Option<HashMap<String, String>>,
}

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateJobRequest {
    pub name: Option<String>,
    #[serde(default, deserialize_with = "some_option")]
    pub schedule_cron: Option<Option<String>>, // Missing is None, Null is Some(None), Value is Some(Some(value))
    pub runner_config: Option<RunnerConfig>,
    pub max_concurrency: Option<u32>,
    pub misfire_policy: Option<MisfirePolicy>,
    /// If present, replace the job's retry policy.
    pub retry: Option<RetryConfig>,
    /// If present, replace the job's environment variables (replace-all).
    pub env: Option<HashMap<String, String>>,
}
