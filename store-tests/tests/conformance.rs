//! Conformance runner: crosses every registered backend with every applicable
//! case via libtest-mimic, so `cargo test` reports each `backend::group::case`.
//!
//! This is the higher-level wiring layer. Backends (and their setup, like the
//! Postgres container/database) live here, never inside the backend crates or the
//! shared suite.

use std::sync::Arc;

use arbiter_store_pg::PgStore;
use arbiter_store_sqlite::SqliteStore;
use arbiter_store_tests::{
    BackendFactory, Capabilities, DurableHandle, StoreRef, cases, durable_cases,
};
use libtest_mimic::{Arguments, Failed, Trial};
use uuid::Uuid;

/// The arbiter schema, applied to each fresh test database.
const SCHEMA_SQL: &str = include_str!("../../docker/init/000_schema.sql");

/// Postgres backend. The single place where PG is set up: it reads
/// `ARBITER_TEST_DATABASE_URL` (or falls back to the dev URL), and each `fresh()`
/// creates a brand-new database, applies the schema, and returns a `PgStore`.
///
/// TODO: when no URL is provided, spin a throwaway container via `testcontainers`
/// instead of falling back to a fixed dev URL.
struct PgBackend {
    /// Connection URL without the trailing `/<database>` component.
    base: String,
}

impl PgBackend {
    fn from_env() -> Self {
        let url = std::env::var("ARBITER_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://arbiter:arbiter@localhost:5432/arbiter".to_string());
        let idx = url.rfind('/').expect("database URL must contain a path");
        Self {
            base: url[..idx].to_string(),
        }
    }
}

#[async_trait::async_trait]
impl BackendFactory for PgBackend {
    fn name(&self) -> &'static str {
        "pg"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            durable: true,
            native_unique: true,
            multi_worker: true,
            leader_election: true,
            multi_node: false, // single-instance test harness; HA tested separately
            fencing: false,
            retention: true,
        }
    }

    async fn fresh(&self) -> StoreRef {
        let db = format!("conf_{}", Uuid::new_v4().simple());

        let admin = sqlx::PgPool::connect(&format!("{}/postgres", self.base))
            .await
            .expect("connect to maintenance database");
        sqlx::raw_sql(&format!("CREATE DATABASE \"{db}\""))
            .execute(&admin)
            .await
            .expect("create test database");
        admin.close().await;

        let url = format!("{}/{}", self.base, db);
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("connect to fresh database");
        sqlx::raw_sql(SCHEMA_SQL)
            .execute(&pool)
            .await
            .expect("apply schema");
        pool.close().await;

        let store = PgStore::new(&url).await.expect("PgStore::new");
        Arc::new(store) as StoreRef
    }

    async fn durable_handle(&self) -> Option<Box<dyn DurableHandle>> {
        let db = format!("conf_{}", Uuid::new_v4().simple());
        let admin = sqlx::PgPool::connect(&format!("{}/postgres", self.base))
            .await
            .expect("connect to maintenance database");
        sqlx::raw_sql(&format!("CREATE DATABASE \"{db}\""))
            .execute(&admin)
            .await
            .expect("create test database");
        admin.close().await;
        let url = format!("{}/{}", self.base, db);
        let pool = sqlx::PgPool::connect(&url)
            .await
            .expect("connect to fresh database");
        sqlx::raw_sql(SCHEMA_SQL)
            .execute(&pool)
            .await
            .expect("apply schema");
        pool.close().await;
        Some(Box::new(PgDurable { url }))
    }
}

struct PgDurable {
    url: String,
}

#[async_trait::async_trait]
impl DurableHandle for PgDurable {
    async fn open(&self) -> StoreRef {
        Arc::new(PgStore::new(&self.url).await.expect("PgStore::new")) as StoreRef
    }
}

/// Embedded SQLite backend. Each `fresh()` opens a new temp-file database (so it is
/// durable and isolated), with no external service needed.
struct SqliteBackend;

#[async_trait::async_trait]
impl BackendFactory for SqliteBackend {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            durable: true,
            native_unique: true,
            multi_worker: true,
            leader_election: true,
            multi_node: false,
            fencing: false,
            retention: true,
        }
    }

    async fn fresh(&self) -> StoreRef {
        let path = std::env::temp_dir().join(format!("arbiter_conf_{}.db", Uuid::new_v4().simple()));
        let store = SqliteStore::connect(path.to_str().expect("utf-8 temp path"))
            .await
            .expect("SqliteStore::connect");
        Arc::new(store) as StoreRef
    }

    async fn durable_handle(&self) -> Option<Box<dyn DurableHandle>> {
        let path = std::env::temp_dir().join(format!("arbiter_dur_{}.db", Uuid::new_v4().simple()));
        Some(Box::new(SqliteDurable {
            path: path.to_str().expect("utf-8 temp path").to_string(),
        }))
    }
}

struct SqliteDurable {
    path: String,
}

#[async_trait::async_trait]
impl DurableHandle for SqliteDurable {
    async fn open(&self) -> StoreRef {
        Arc::new(
            SqliteStore::connect(&self.path)
                .await
                .expect("SqliteStore::connect"),
        ) as StoreRef
    }
}

fn main() {
    let args = Arguments::from_args();

    let backends: Vec<Box<dyn BackendFactory>> =
        vec![Box::new(PgBackend::from_env()), Box::new(SqliteBackend)];

    let mut trials = Vec::new();
    for b in backends {
        let b: Arc<dyn BackendFactory> = Arc::from(b);
        let caps = b.capabilities();
        for c in cases() {
            let name = format!("{}::{}::{}", b.name(), c.group, c.name);
            if caps.satisfies(c.needs) {
                let b = b.clone();
                let run = c.run;
                trials.push(Trial::test(name, move || {
                    let rt = tokio::runtime::Runtime::new()
                        .map_err(|e| Failed::from(e.to_string()))?;
                    rt.block_on(async move {
                        let store = b.fresh().await;
                        run(store).await;
                    });
                    Ok(())
                }));
            } else {
                trials.push(Trial::test(name, || Ok::<_, Failed>(())).with_ignored_flag(true));
            }
        }

        // Durability cases get a reopenable handle instead of a single store.
        if caps.durable {
            for dc in durable_cases() {
                let name = format!("{}::{}::{}", b.name(), dc.group, dc.name);
                let b = b.clone();
                let run = dc.run;
                trials.push(Trial::test(name, move || {
                    let rt = tokio::runtime::Runtime::new()
                        .map_err(|e| Failed::from(e.to_string()))?;
                    rt.block_on(async move {
                        let handle = b
                            .durable_handle()
                            .await
                            .expect("durable backend must provide a durable_handle");
                        run(handle).await;
                    });
                    Ok(())
                }));
            }
        }
    }

    libtest_mimic::run(&args, trials).exit();
}
