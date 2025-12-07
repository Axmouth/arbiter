use dromio_config::NodeConfig;
use dromio_core::{DromioError, WorkerStore};
use dromio_core::{Result, SchedulerConfig, WorkerConfig};
use dromio_scheduler::run_scheduler_loop;
use dromio_store_pg::PgStore;
use dromio_worker::run_worker_loop;
use fd_lock::{RwLock, RwLockWriteGuard};
use std::path::PathBuf;
use std::{path::Path, sync::Arc};
use tokio::fs::{File, OpenOptions};
use tokio::{fs, io::AsyncWriteExt};
use uuid::Uuid;

struct WorkerIdentity {
    id: Uuid,
    display_name: String,
    restart_count: u32,
    file_path: PathBuf,
}

impl WorkerIdentity {
    fn short_id(&self) -> String {
        self.id.to_string()[0..8].to_string()
    }
}

// TODO: Local node config management through admin panel. Maybe node has its own dashboard too?
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(true)
        .with_max_level(tracing::Level::INFO)
        .init();

    let cfg = NodeConfig::try_load()?;

    let store = Arc::new(PgStore::new(&cfg.database.url).await?);

    let hostname_str = hostname::get()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let allow_multi = std::env::var("DROMIO_ALLOW_MULTI_ID")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let (identity, mut lock) = load_or_register_identity(
        store.as_ref(),
        &hostname_str,
        env!("CARGO_PKG_VERSION"),
        allow_multi,
    )
    .await?;

    // Hold guard for whole process
    let _guard = lock.try_write().expect("failed to lock identity file");

    tracing::debug!("identity file locked at {}", identity.file_path.display());

    // worker config
    let worker_cfg = WorkerConfig {
        worker_id: identity.id,
        display_name: identity.display_name.clone(),
        hostname: hostname_str,
        capacity: 4,
        tick_interval_ms: 200,
        heartbeat_interval_ms: 2_000,
        dead_after_secs: 30,
        restart_count: identity.restart_count,
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let scheduler_cfg = SchedulerConfig {
        tick_interval_ms: 2_000,
    };

    tracing::info!(
        "🚀 Worker '{}' [{}…] starting (#{}), file={}, host={}, v{}",
        identity.display_name,
        identity.short_id(),
        identity.restart_count,
        identity.file_path.display(),
        worker_cfg.hostname,
        worker_cfg.version,
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

async fn load_or_register_identity(
    store: &PgStore,
    hostname: &str,
    version: &str,
    allow_multi: bool,
) -> Result<(WorkerIdentity, RwLock<File>)> {
    let (lock, path) = if allow_multi {
        acquire_identity_file()
            .await
            .map_err(|e| DromioError::ExecutionError(e.to_string()))?
    } else {
        // Strict single identity
        let path = PathBuf::from("/data/worker-id");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .await
            .map_err(|e| DromioError::ExecutionError(e.to_string()))?;
        let mut lock = RwLock::new(file);
        if let Err(_e) = lock.try_write() {
            tracing::error!(
                "Another worker already uses this identity file: {}",
                path.display()
            );
            std::process::exit(1);
        };
        (lock, path)
    };

    // Try read existing UUID
    let uuid_opt = tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| Uuid::parse_str(s.trim()).ok());

    if let Some(id) = uuid_opt {
        if let Some((display_name, _)) = store.lookup_by_id(id).await? {
            let restart_count = store.incr_restart_count(id, version).await?;

            tracing::info!(
                "Recovered identity '{} [{}…]' restart #{} from {}",
                display_name,
                &id.to_string()[0..8],
                restart_count,
                path.display()
            );

            return Ok((
                WorkerIdentity {
                    id,
                    display_name,
                    restart_count,
                    file_path: path,
                },
                lock,
            ));
        }

        tracing::warn!(
            "Identity file {} exists but DB has no record. Assuming stale ID.",
            path.display()
        );
    }

    // New identity case
    let id = Uuid::new_v4();
    let display_name = uuid_readable_rs::generate_from(id);
    let restart_count = 1;

    store
        .insert_worker(id, &display_name, hostname, version, restart_count)
        .await?;
    persist_uuid_to_file(id, &path)
        .await
        .map_err(|e| DromioError::ExecutionError(e.to_string()))?;

    tracing::info!(
        "New identity '{}' [{}…] created at {}",
        display_name,
        &id.to_string()[0..8],
        path.display()
    );

    Ok((
        WorkerIdentity {
            id,
            display_name,
            restart_count,
            file_path: path,
        },
        lock,
    ))
}

async fn persist_uuid_to_file(id: Uuid, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let mut file = File::create(path).await?;
    file.write_all(id.to_string().as_bytes()).await?;
    file.flush().await?;
    Ok(())
}

async fn acquire_identity_file() -> std::io::Result<(RwLock<File>, PathBuf)> {
    let base = Path::new("/data/worker-id");

    for i in 0..100 {
        let path = if i == 0 {
            base.to_owned()
        } else {
            base.with_extension(i.to_string()) // worker-id.2
        };

        // ensure dir exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .await?;

        let mut lock = RwLock::new(file);

        // Try exclusive lock
        if lock.try_write().is_ok() {
            return Ok((lock, path));
        }

        // else file locked — try next suffix
    }

    Err(std::io::Error::other("Exhausted worker-id slots"))
}
