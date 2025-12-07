use chrono::{DateTime, Utc};
use serde::Deserialize;
use ts_rs::TS;
use utoipa::ToSchema;

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRunsQuery {
    #[ts(optional)]
    pub before: Option<DateTime<Utc>>,
    #[ts(optional)]
    pub after: Option<DateTime<Utc>>,
    #[ts(optional)]
    pub by_job_id: Option<uuid::Uuid>,
    #[ts(optional)]
    pub by_worker_id: Option<uuid::Uuid>,
    #[ts(optional)]
    pub limit: Option<u32>,
}
