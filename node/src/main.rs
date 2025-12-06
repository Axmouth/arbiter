use dromio_config::NodeConfig;
use dromio_core::{Result, SchedulerConfig, WorkerConfig};
use dromio_scheduler::run_scheduler_loop;
use dromio_store_pg::PgStore;
use dromio_worker::run_worker_loop;
use std::sync::Arc;
use uuid::Uuid;
use uuid_readable_rs::generate_from;

// TODO: Local node config management through admin panel. Maybe node has its own dashboard too?
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(true)
        .with_max_level(tracing::Level::INFO)
        .init();

    let cfg = NodeConfig::try_load()?;

    let store = Arc::new(PgStore::new(&cfg.database.url).await?);

    // worker config
    let worker_id = Uuid::new_v4();
    let worker_cfg = WorkerConfig {
        worker_id,
        display_name: generate_from(worker_id),
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

    tracing::info!(
        "Dromio Node starting.. Id={{{}}} Readable Name={{{}}}",
        worker_cfg.worker_id,
        worker_cfg.display_name
    );

    let store_for_scheduler = store.clone();
    let store_for_worker = store.clone();

    // Scheduler task
    tokio::spawn(async move {
        run_scheduler_loop(store_for_scheduler, scheduler_cfg, worker_cfg.worker_id).await;
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
