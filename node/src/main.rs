use dromio_core::{
    DromioError, JobRun, JobRunState, JobStore, Result, RunStore, SchedulerConfig, Store,
    WorkerConfig, WorkerRecord, WorkerStore,
};
use dromio_scheduler::run_scheduler_loop;
use dromio_store_pg::PgStore;
use dromio_worker::run_worker_loop;
use shell_words::split;
use std::sync::Arc;
use tokio::process::Command;
use tokio::time::sleep;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    // TODO: use config/env; hardcoded for now
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://dromio:dromio@localhost:2345/dromio".into());

    let store = Arc::new(PgStore::new(&database_url).await?);

    // worker config
    let worker_cfg = WorkerConfig {
        worker_id: Uuid::new_v4(),
        hostname: hostname::get()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        capacity: 4,
        tick_interval_ms: 200,
        heartbeat_interval_ms: 2_000,
        dead_after_secs: 30,
    };

    let scheduler_cfg = SchedulerConfig {
        tick_interval_ms: 2_000,
    };

    let store_for_scheduler = store.clone();
    let store_for_worker = store.clone();

    // Scheduler task
    tokio::spawn(async move {
        run_scheduler_loop(store_for_scheduler, scheduler_cfg).await;
    });

    // Worker task
    tokio::spawn(async move {
        run_worker_loop(store_for_worker, worker_cfg).await;
    });

    // Later: HTTP API here (axum server)
    // For now, just park the main task:
    futures::future::pending::<()>().await;
    Ok(())
}
