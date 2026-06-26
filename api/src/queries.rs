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

/// Paginated read of a run's log chunks. `after` reads forward from a cursor (catch up to
/// live), `before` reads the newest chunks below a seq (tail, then page earlier). With
/// neither, the latest `limit` chunks are returned (the tail).
#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunLogQuery {
    #[ts(optional)]
    pub after: Option<i64>,
    #[ts(optional)]
    pub before: Option<i64>,
    #[ts(optional)]
    pub limit: Option<u32>,
}

#[derive(Deserialize, TS, ToSchema)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PruneRunsQuery {
    /// Delete terminal runs older than this many days (default 30).
    #[ts(optional)]
    pub older_than_days: Option<u32>,
}
