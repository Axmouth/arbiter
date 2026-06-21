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
    BackendFactory, Capabilities, DurableHandle, StoreRef, cases, durable_cases, leadership_cases,
};
use libtest_mimic::{Arguments, Failed, Trial};
use uuid::Uuid;

/// The arbiter schema, applied to each fresh test database.
const SCHEMA_SQL: &str = include_str!("../../docker/init/000_schema.sql");

/// Postgres backend. The single place where PG is set up: it uses
/// `ARBITER_TEST_DATABASE_URL` if set, otherwise spins a throwaway Postgres
/// container for the test process (so local `cargo test` needs no manual setup).
/// Each `fresh()` creates a brand-new database, applies the schema, and returns a
/// `PgStore`.
struct PgBackend {
    /// Connection URL without the trailing `/<database>` component.
    base: String,
}

impl PgBackend {
    async fn new() -> Self {
        if let Ok(url) = std::env::var("ARBITER_TEST_DATABASE_URL") {
            let idx = url.rfind('/').expect("database URL must contain a path");
            return Self {
                base: url[..idx].to_string(),
            };
        }

        use testcontainers::runners::AsyncRunner;
        let container = testcontainers_modules::postgres::Postgres::default()
            .start()
            .await
            .expect("start postgres container");
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("postgres container port");
        // Keep the container running for the whole test process; the testcontainers
        // reaper removes it when the session ends.
        std::mem::forget(container);
        Self {
            base: format!("postgres://postgres:postgres@localhost:{port}"),
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

    async fn paired(&self) -> Option<(StoreRef, StoreRef)> {
        let url = pg_fresh_db(&self.base).await;
        let a = Arc::new(PgStore::new(&url).await.expect("PgStore::new")) as StoreRef;
        let b = Arc::new(PgStore::new(&url).await.expect("PgStore::new")) as StoreRef;
        Some((a, b))
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

/// Build a fresh, schema-loaded Postgres database and return its URL.
async fn pg_fresh_db(base: &str) -> String {
    let db = format!("conf_{}", Uuid::new_v4().simple());
    let admin = sqlx::PgPool::connect(&format!("{base}/postgres"))
        .await
        .expect("connect to maintenance database");
    sqlx::raw_sql(&format!("CREATE DATABASE \"{db}\""))
        .execute(&admin)
        .await
        .expect("create test database");
    admin.close().await;
    let url = format!("{base}/{db}");
    let pool = sqlx::PgPool::connect(&url)
        .await
        .expect("connect to fresh database");
    sqlx::raw_sql(SCHEMA_SQL)
        .execute(&pool)
        .await
        .expect("apply schema");
    pool.close().await;
    url
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
        let path =
            std::env::temp_dir().join(format!("arbiter_conf_{}.db", Uuid::new_v4().simple()));
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

    async fn paired(&self) -> Option<(StoreRef, StoreRef)> {
        let path =
            std::env::temp_dir().join(format!("arbiter_pair_{}.db", Uuid::new_v4().simple()));
        let path = path.to_str().expect("utf-8 temp path").to_string();
        let a = Arc::new(
            SqliteStore::connect(&path)
                .await
                .expect("SqliteStore::connect"),
        ) as StoreRef;
        let b = Arc::new(
            SqliteStore::connect(&path)
                .await
                .expect("SqliteStore::connect"),
        ) as StoreRef;
        Some((a, b))
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

    // Bootstrap runtime: build the Postgres backend (which may start a container).
    let rt = tokio::runtime::Runtime::new().expect("bootstrap runtime");
    let pg = rt.block_on(PgBackend::new());

    let backends: Vec<Box<dyn BackendFactory>> =
        vec![Box::new(pg), Box::new(SqliteBackend)];

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
                    let rt =
                        tokio::runtime::Runtime::new().map_err(|e| Failed::from(e.to_string()))?;
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
                    let rt =
                        tokio::runtime::Runtime::new().map_err(|e| Failed::from(e.to_string()))?;
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

        // Leadership cases need two handles to the same backend instance.
        if caps.leader_election {
            for lc in leadership_cases() {
                let name = format!("{}::{}::{}", b.name(), lc.group, lc.name);
                let b = b.clone();
                let run = lc.run;
                trials.push(Trial::test(name, move || {
                    let rt =
                        tokio::runtime::Runtime::new().map_err(|e| Failed::from(e.to_string()))?;
                    rt.block_on(async move {
                        let pair = b
                            .paired()
                            .await
                            .expect("leader_election backend must provide paired()");
                        run(pair).await;
                    });
                    Ok(())
                }));
            }
        }
    }

    libtest_mimic::run(&args, trials).exit();
}
