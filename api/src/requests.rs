use dromio_core::{MisfirePolicy, RunnerConfig};
use serde::{Deserialize, Deserializer};
use ts_rs::TS;
use utoipa::ToSchema;

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
}

