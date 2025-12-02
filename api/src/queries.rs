use chrono::{DateTime, Utc};
use serde::Deserialize;
use ts_rs::TS;
use utoipa::ToSchema;

#[derive(Deserialize, TS, ToSchema)]
#[ts(export)]
pub struct ListRunsQuery {
    #[ts(optional)]
    pub before: Option<DateTime<Utc>>,
    #[ts(optional)]
    pub after: Option<DateTime<Utc>>,
    #[ts(optional)]
    pub by_job_id: Option<uuid::Uuid>,
}
