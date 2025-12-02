use std::{result, str::FromStr, sync::Arc};

use chrono::{DateTime, Duration, DurationRound, Utc};
use croner::{Cron, Direction};
use dromio_core::{DromioError, JobStore, Result, RunStore, SchedulerConfig};
use tokio::time::sleep;

pub async fn run_scheduler_loop<S>(store: Arc<S>, cfg: SchedulerConfig) -> !
where
    S: JobStore + RunStore + Send + Sync + 'static,
{
    loop {
        let now = Utc::now();

        // TODO: later gate this on leader election:
        // if !leader_elector.am_i_leader().await { sleep(...); continue; }

        if let Err(e) = scheduler_tick(store.as_ref(), now).await {
            // For now just log; later use tracing
            eprintln!("[scheduler] tick error: {e:?}");
        }

        sleep(std::time::Duration::from_millis(cfg.tick_interval_ms)).await;
    }
}

pub async fn scheduler_tick(store: &(impl JobStore + RunStore), now: DateTime<Utc>) -> Result<()> {
    let jobs = store.list_enabled_cron_jobs().await?;

    let jobs_num = jobs.len();
    let mut jobs_scheduled = 0;

    for job in jobs {
        if let Some(cron) = &job.schedule_cron {
            let next =
                if let Ok(next) = compute_next_fire_times(cron, now, now + Duration::minutes(1)) {
                    next
                } else {
                    // Should not happen in any reasonable setup
                    eprintln!(
                        "[scheduler] invalid cron expression for job {}: {}",
                        job.id, cron
                    );
                    continue;
                };

            // TODO: batch/parallel insert?
            for ts in next {
                // TODO: Keep a rolling cache of already-inserted runs to avoid DB hits?
                let result = store
                    .insert_job_run_if_missing(job.id, ts, &job.command)
                    .await;
                if let Err(e) = result {
                    eprintln!(
                        "[scheduler] failed to insert job run for job {} at {}: {:?}",
                        job.id, ts, e
                    );
                    continue;
                } else if let Ok(inserted) = result
                    && !inserted
                {
                    // already existed
                    continue;
                }
                jobs_scheduled += 1;
            }
        }
    }

    if jobs_num > 0 || jobs_scheduled > 0 {
        println!(
            "[scheduler] tick at {}, processed {} jobs, scheduled {} runs",
            now, jobs_num, jobs_scheduled
        );
    }

    Ok(())
}

fn compute_next_fire_times(
    cron: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<DateTime<Utc>>> {
    let schedule = Cron::from_str(cron).map_err(|e| DromioError::InvalidInput(e.to_string()))?;
    let times = schedule
        .clone()
        .iter_from(start, Direction::Forward)
        .take_while(|t| *t <= end)
        // TODO: make cleaner?
        .filter_map(|ts| {
            if let Ok(ts) = ts.duration_trunc(Duration::seconds(1)) {
                Some(ts)
            } else {
                eprintln!("[scheduler] failed to truncate time {}", ts);
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
        let times = compute_next_fire_times("* * * * *", start, end).unwrap();

        assert_eq!(times.len(), 6);
        assert_eq!(times[0], start);
        assert_eq!(times[5], end);
    }

    #[test]
    fn test_hour_rollover() {
        let start = Utc.with_ymd_and_hms(2025, 1, 1, 1, 58, 0).unwrap();
        let end = Utc.with_ymd_and_hms(2025, 1, 1, 2, 2, 0).unwrap();
        let times = compute_next_fire_times("*/2 * * * *", start, end).unwrap();

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

        let times = compute_next_fire_times("0 0 * * Mon", start, end).unwrap();

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
        let times = compute_next_fire_times("0 * * * *", start, end).unwrap();

        assert_eq!(times, vec![start, end]);
    }

    #[test]
    fn test_invalid_cron() {
        let start = Utc::now();
        let end = start + chrono::Duration::hours(1);

        let err = compute_next_fire_times("NOT A CRON", start, end).unwrap_err();

        match err {
            DromioError::InvalidInput(_) => {}
            _ => panic!("Unexpected error type"),
        }
    }
}
