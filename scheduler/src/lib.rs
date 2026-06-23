use std::{str::FromStr, sync::Arc};

use chrono::{DateTime, Duration, DurationRound, Utc};
use croner::{Cron, Direction};
use arbiter_core::{
    ArbiterError, JobStore, MisfirePolicy, Result, RunStore, RuntimeSettings, SchedulerConfig,
    WorkerStore, snooze,
};
use uuid::Uuid;

pub async fn run_scheduler_loop<S>(
    store: Arc<S>,
    cfg: SchedulerConfig,
    worker_id: Uuid,
    settings: Arc<RuntimeSettings>,
) -> !
where
    S: JobStore + RunStore + WorkerStore + Send + Sync + 'static,
{
    loop {
        let now = Utc::now();

        // TODO: Investigate caching jobs and invalidating on update
        if let Err(e) = scheduler_tick(store.as_ref(), now, worker_id, &settings).await {
            // For now just log; later use tracing
            tracing::error!("{worker_id}: tick error: {e:?}");
        }

        snooze(std::time::Duration::from_millis(cfg.tick_interval_ms), 30).await;
    }
}

pub async fn scheduler_tick(
    store: &(impl JobStore + RunStore + WorkerStore + Send + Sync),
    now: DateTime<Utc>,
    worker_id: Uuid,
    settings: &RuntimeSettings,
) -> Result<()> {
    if !store.am_i_leader().await? {
        return Ok(());
    }

    // Runtime setting overrides the static config default (near-live via the cache).
    let catchup = Duration::seconds(settings.misfire_catchup_secs() as i64);

    let jobs = store.list_enabled_cron_jobs().await?;

    let jobs_num = jobs.len();
    let mut jobs_scheduled = 0;
    let lookahead = now + Duration::minutes(1);

    for job in jobs {
        let Some(cron) = &job.schedule_cron else {
            continue;
        };

        // Look back far enough to catch misfires (bounded by the job's policy), then
        // ahead for normal scheduling.
        let start = now - misfire_lookback(&job.misfire_policy, catchup);
        let fires = match compute_next_fire_times(cron, start, lookahead, worker_id) {
            Ok(f) => f,
            Err(_) => {
                tracing::error!(
                    "{worker_id}: invalid cron expression for job {}: {}",
                    job.id,
                    cron
                );
                continue;
            }
        };

        // Future fires materialize normally; missed (past) fires follow the policy.
        let (past, future): (Vec<DateTime<Utc>>, Vec<DateTime<Utc>>) =
            fires.into_iter().partition(|ts| *ts < now);
        let missed = select_misfire_fires(&job.misfire_policy, &past, now);

        // TODO: batch/parallel insert? Keep a rolling cache to avoid DB hits?
        for ts in future.into_iter().chain(missed) {
            match store.insert_job_run_if_missing(job.id, ts).await {
                Ok(true) => jobs_scheduled += 1,
                Ok(false) => {} // already existed
                Err(e) => tracing::error!(
                    "{worker_id}: failed to insert job run for job {} at {}: {e:?}",
                    job.id,
                    ts
                ),
            }
        }
    }

    if jobs_num > 0 || jobs_scheduled > 0 {
        tracing::info!(
            "{worker_id}: tick at {}, processed {} jobs, scheduled {} runs",
            now,
            jobs_num,
            jobs_scheduled
        );
    }

    Ok(())
}

/// How far back to scan for missed fires, bounded by the global catch-up window.
/// `catchup` of zero disables backfill entirely (only future fires materialize).
fn misfire_lookback(policy: &MisfirePolicy, catchup: Duration) -> Duration {
    match policy {
        MisfirePolicy::Skip => Duration::zero(),
        // Self-bounded: its own window applies regardless of the global cap, so a
        // per-job RunIfLateWithin works without an operator enabling catch-up.
        MisfirePolicy::RunIfLateWithin(d) => *d,
        // Unbounded by nature: bounded by the global catch-up cap (0 = no backfill).
        MisfirePolicy::RunAll | MisfirePolicy::Coalesce | MisfirePolicy::RunImmediately => catchup,
    }
}

/// Which missed (past) fire times to materialize, per the job's misfire policy.
/// `past` is ascending; active runs are filtered later by the idempotent insert.
fn select_misfire_fires(
    policy: &MisfirePolicy,
    past: &[DateTime<Utc>],
    now: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    match policy {
        MisfirePolicy::Skip => Vec::new(),
        MisfirePolicy::RunAll => past.to_vec(),
        MisfirePolicy::RunIfLateWithin(d) => {
            past.iter().filter(|ts| now - **ts <= *d).copied().collect()
        }
        // Collapse a gap of missed fires into a single run (the most recent).
        MisfirePolicy::Coalesce | MisfirePolicy::RunImmediately => {
            past.last().copied().into_iter().collect()
        }
    }
}

fn compute_next_fire_times(
    cron: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    worker_id: Uuid,
) -> Result<Vec<DateTime<Utc>>> {
    let schedule = Cron::from_str(cron).map_err(|e| ArbiterError::InvalidInput(e.to_string()))?;
    let times = schedule
        .clone()
        .iter_from(start, Direction::Forward)
        .take_while(|t| *t <= end)
        // TODO: make cleaner?
        .filter_map(|ts| {
            if let Ok(ts) = ts.duration_trunc(Duration::seconds(1)) {
                Some(ts)
            } else {
                tracing::error!("{worker_id}: failed to truncate time {}", ts);
                None
            }
        })
        .collect::<Vec<_>>();
    Ok(times)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_every_minute() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 1, 0, 5, 0).unwrap();
        let times = compute_next_fire_times("* * * * *", start, end, Uuid::new_v4()).unwrap();

        assert_eq!(times.len(), 6);
        assert_eq!(times[0], start);
        assert_eq!(times[5], end);
    }

    #[test]
    fn test_hour_rollover() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 1, 58, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 1, 2, 2, 0).unwrap();
        let times = compute_next_fire_times("*/2 * * * *", start, end, Uuid::new_v4()).unwrap();

        assert_eq!(
            times,
            vec![
                Utc.with_ymd_and_hms(2025, 1, 1, 1, 58, 0).unwrap(),
                Utc.with_ymd_and_hms(2025, 1, 1, 2, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2025, 1, 1, 2, 2, 0).unwrap(),
            ]
        );
    }

    #[test]
    fn test_dow_and_month_boundary() {
        // Let's say this Thursday
        let start = Utc.with_ymd_and_hms(2025, 1, 30, 23, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 2, 3, 1, 0, 0).unwrap();

        let times = compute_next_fire_times("0 0 * * Mon", start, end, Uuid::new_v4()).unwrap();

        // First Monday is Feb 3, 2025
        assert_eq!(
            times,
            vec![Utc.with_ymd_and_hms(2025, 2, 3, 0, 0, 0).unwrap()]
        );
    }

    #[test]
    fn test_end_inclusive() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 1, 1, 0, 0).unwrap();
        let times = compute_next_fire_times("0 * * * *", start, end, Uuid::new_v4()).unwrap();

        assert_eq!(times, vec![start, end]);
    }

    #[test]
    fn test_invalid_cron() {
        let start = Utc::now();
        let end = start + chrono::Duration::hours(1);

        let err = compute_next_fire_times("NOT A CRON", start, end, Uuid::new_v4()).unwrap_err();

        match err {
            ArbiterError::InvalidInput(_) => {}
            _ => panic!("Unexpected error type"),
        }
    }

    #[test]
    fn misfire_skip_materializes_no_missed() {
        let now = Utc::now();
        let past = [now - Duration::minutes(2), now - Duration::minutes(1)];
        assert!(select_misfire_fires(&MisfirePolicy::Skip, &past, now).is_empty());
    }

    #[test]
    fn misfire_run_all_materializes_every_missed() {
        let now = Utc::now();
        let past = [now - Duration::minutes(2), now - Duration::minutes(1)];
        assert_eq!(
            select_misfire_fires(&MisfirePolicy::RunAll, &past, now),
            past.to_vec()
        );
    }

    #[test]
    fn misfire_coalesce_materializes_latest_only() {
        let now = Utc::now();
        let older = now - Duration::minutes(2);
        let latest = now - Duration::minutes(1);
        assert_eq!(
            select_misfire_fires(&MisfirePolicy::Coalesce, &[older, latest], now),
            vec![latest]
        );
    }

    #[test]
    fn misfire_run_if_late_filters_by_window() {
        let now = Utc::now();
        let recent = now - Duration::minutes(1);
        let old = now - Duration::minutes(10);
        assert_eq!(
            select_misfire_fires(
                &MisfirePolicy::RunIfLateWithin(Duration::minutes(5)),
                &[old, recent],
                now
            ),
            vec![recent]
        );
    }

    #[test]
    fn misfire_lookback_cap_bounds_only_unbounded_policies() {
        // The cap bounds the unbounded family (RunAll here): 0 cap -> no backfill.
        assert_eq!(
            misfire_lookback(&MisfirePolicy::RunAll, Duration::zero()),
            Duration::zero()
        );
        // Skip never looks back regardless of catch-up.
        assert_eq!(
            misfire_lookback(&MisfirePolicy::Skip, Duration::minutes(30)),
            Duration::zero()
        );
        // RunIfLateWithin self-bounds by its own window even when the cap is 0.
        assert_eq!(
            misfire_lookback(
                &MisfirePolicy::RunIfLateWithin(Duration::minutes(5)),
                Duration::zero()
            ),
            Duration::minutes(5)
        );
    }
}
