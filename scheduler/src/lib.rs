use std::{str::FromStr, sync::Arc};

use chrono::{DateTime, Duration, DurationRound, Utc};
use croner::{Cron, Direction};
use arbiter_core::{
    ArbiterError, Clock, JobStore, MisfirePolicy, Result, RuntimeSettings, SchedulerConfig,
    WorkerStore, jittered_backstop_secs, snooze,
};
use uuid::Uuid;

/// How far ahead the leader materializes fires each pass. Anything within this window of
/// now is already inserted, so it doubles as the lead: the leader only needs to wake again
/// when the next *un-materialized* fire is this close.
const LOOKAHEAD_SECS: i64 = 60;

pub async fn run_scheduler_loop<S>(
    store: Arc<S>,
    cfg: SchedulerConfig,
    worker_id: Uuid,
    settings: Arc<RuntimeSettings>,
    clock: Arc<dyn Clock>,
) -> !
where
    S: JobStore + WorkerStore + Send + Sync + 'static,
{
    loop {
        let now = clock.now();

        let leader = match store.am_i_leader().await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("{worker_id}: leadership check failed: {e:?}");
                false
            }
        };

        // Followers re-check leadership on a short fixed cadence so failover stays fast
        // (independent of the leader's long, backstop-bounded planning sleep).
        if !leader {
            snooze(std::time::Duration::from_millis(cfg.tick_interval_ms), 30).await;
            continue;
        }

        // Plan: materialize what's due/imminent and learn the next un-materialized fire.
        let next_fire = match scheduler_tick(store.as_ref(), now, worker_id, &settings).await {
            Ok(nf) => nf,
            Err(e) => {
                tracing::error!("{worker_id}: tick error: {e:?}");
                None
            }
        };

        // Sleep until that fire approaches, capped by the backstop, but wake immediately
        // if a job change invalidates the plan.
        let backstop = jittered_backstop_secs(settings.scheduler_backstop_secs(), 15);
        let wake = next_wake(now, next_fire, backstop);
        let sleep_for = (wake - now).to_std().unwrap_or(std::time::Duration::ZERO);
        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = store.await_jobs_change() => {}
        }
    }
}

/// Materialize due/imminent runs for the leader and return the earliest fire beyond the
/// lookahead window (the next one not yet materialized), or `None` if no enabled cron
/// jobs. Assumes the caller has already confirmed leadership.
pub async fn scheduler_tick(
    store: &(impl JobStore + Send + Sync),
    now: DateTime<Utc>,
    worker_id: Uuid,
    settings: &RuntimeSettings,
) -> Result<Option<DateTime<Utc>>> {
    // Runtime setting overrides the static config default (near-live via the cache).
    let catchup = Duration::seconds(settings.misfire_catchup_secs() as i64);

    let jobs = store.list_enabled_cron_jobs().await?;

    let jobs_num = jobs.len();
    let mut jobs_scheduled = 0;
    let mut earliest_next: Option<DateTime<Utc>> = None;
    let lookahead = now + Duration::seconds(LOOKAHEAD_SECS);

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

        // The earliest fire beyond the lookahead window is the next thing to wake for.
        if let Some(nf) = next_fire_after(cron, lookahead) {
            earliest_next = Some(earliest_next.map_or(nf, |cur| cur.min(nf)));
        }
    }

    if jobs_num > 0 || jobs_scheduled > 0 {
        tracing::info!(
            "{worker_id}: planned at {}, {} jobs, scheduled {} runs, next fire {:?}",
            now,
            jobs_num,
            jobs_scheduled,
            earliest_next,
        );
    }

    Ok(earliest_next)
}

/// The next fire strictly after `after`, or `None` if the cron never fires again / is
/// invalid (invalid crons are surfaced elsewhere; here we just skip them for planning).
fn next_fire_after(cron: &str, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let schedule = Cron::from_str(cron).ok()?;
    schedule
        .iter_from(after, Direction::Forward)
        .filter_map(|ts| ts.duration_trunc(Duration::seconds(1)).ok())
        .find(|ts| *ts > after)
}

/// When the leader should next wake: as the next fire approaches (minus the lookahead
/// lead), but never later than the backstop. `backstop_secs == 0` means unbounded (sleep
/// to the next fire, relying on a change notification); with no jobs and no backstop we
/// park for a long time and wait to be notified. Never returns a time in the past.
fn next_wake(
    now: DateTime<Utc>,
    next_fire: Option<DateTime<Utc>>,
    backstop_secs: u64,
) -> DateTime<Utc> {
    let lead = Duration::seconds(LOOKAHEAD_SECS);
    let by_fire = next_fire.map(|f| f - lead);
    let by_backstop =
        (backstop_secs > 0).then(|| now + Duration::seconds(backstop_secs as i64));
    let wake = match (by_fire, by_backstop) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => now + Duration::days(1),
    };
    wake.max(now)
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
    fn next_fire_after_is_strictly_after() {
        let after = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 30).unwrap();
        let nf = next_fire_after("* * * * *", after).unwrap();
        assert_eq!(nf, Utc.with_ymd_and_hms(2025, 1, 1, 0, 1, 0).unwrap());
    }

    #[test]
    fn next_wake_caps_far_fire_at_backstop() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let fire = now + Duration::hours(1);
        assert_eq!(
            next_wake(now, Some(fire), 180),
            now + Duration::seconds(180)
        );
    }

    #[test]
    fn next_wake_targets_imminent_fire_minus_lead() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        // 90s out; lead is 60s, so wake 30s from now (before the backstop).
        let fire = now + Duration::seconds(90);
        assert_eq!(next_wake(now, Some(fire), 180), now + Duration::seconds(30));
    }

    #[test]
    fn next_wake_unbounded_sleeps_to_fire_minus_lead() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        let fire = now + Duration::hours(2);
        assert_eq!(next_wake(now, Some(fire), 0), fire - Duration::seconds(60));
    }

    #[test]
    fn next_wake_no_jobs_uses_backstop() {
        let now = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(next_wake(now, None, 180), now + Duration::seconds(180));
        // Unbounded with no jobs parks far out (waiting to be notified), never the past.
        assert!(next_wake(now, None, 0) > now + Duration::hours(1));
    }

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
