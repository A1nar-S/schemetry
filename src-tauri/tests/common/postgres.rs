//! Shared setup for `postgres_integration.rs`: connection details for the two
//! docker-compose Postgres containers, and a raw-SQL script executor.
//!
//! Connection details default to `docker/docker-compose.yml`'s ports/credentials and
//! can be overridden with `SCHEMETRY_TEST_PG_HOST`, `SCHEMETRY_TEST_PG_SOURCE_PORT`,
//! `SCHEMETRY_TEST_PG_TARGET_PORT`, `SCHEMETRY_TEST_PG_DATABASE`,
//! `SCHEMETRY_TEST_PG_USER`, `SCHEMETRY_TEST_PG_PASSWORD`.

use std::env;

use schemetry_lib::models::{ConnectionRecord, DbType};
use tokio_postgres::NoTls;

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn connection(name: &str, default_port: u16, port_env: &str) -> ConnectionRecord {
    let host = env_or("SCHEMETRY_TEST_PG_HOST", "localhost");
    let port: u16 = env::var(port_env)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_port);
    let service_name = env_or("SCHEMETRY_TEST_PG_DATABASE", "schemetry");
    let username = env_or("SCHEMETRY_TEST_PG_USER", "schemetry");
    let password = env_or("SCHEMETRY_TEST_PG_PASSWORD", "SchemetryTest_2024");

    ConnectionRecord {
        id: 0,
        name: name.to_string(),
        db_type: DbType::Postgres,
        host,
        port,
        service_name,
        username,
        password,
        group_name: "Integration Tests".to_string(),
        pg_schema: "public".to_string(),
        password_broken: false,
    }
}

pub fn source_connection() -> ConnectionRecord {
    connection("SOURCE", 5432, "SCHEMETRY_TEST_PG_SOURCE_PORT")
}

pub fn target_connection() -> ConnectionRecord {
    connection("TARGET", 5433, "SCHEMETRY_TEST_PG_TARGET_PORT")
}

/// Splits a fix script into top-level, blank-line-separated chunks and drops any
/// chunk that's comments only. Each chunk may itself contain more than one
/// `;`-terminated statement (e.g. `CREATE TABLE ...;` immediately followed by
/// `COMMENT ON ...;` with no blank line between them) — that's fine, since
/// `batch_execute` runs an arbitrary multi-statement SQL string the same way `psql -f`
/// would, comments included.
fn split_statements(script: &str) -> Vec<String> {
    script
        .split("\n\n")
        .map(|chunk| {
            chunk
                .lines()
                .filter(|l| !l.trim_start().starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Executes a fix script against `conn`, chunk by chunk (see [`split_statements`]),
/// using a private runtime — mirrors how `DbPostgresRepository` bridges async
/// `tokio-postgres` into a synchronous call. Returns how many chunks ran.
pub fn execute_script(conn: &ConnectionRecord, script: &str) -> usize {
    let runtime = tokio::runtime::Runtime::new().expect("failed to create test runtime");
    runtime.block_on(async {
        let mut cfg = tokio_postgres::Config::new();
        cfg.host(&conn.host)
            .port(conn.port)
            .user(&conn.username)
            .password(&conn.password)
            .dbname(&conn.service_name);
        let (client, connection) = cfg.connect(NoTls).await.expect("raw postgres connection failed");
        tokio::spawn(async move {
            let _ = connection.await;
        });

        let mut executed = 0;
        for statement in split_statements(script) {
            client
                .batch_execute(&statement)
                .await
                .unwrap_or_else(|e| panic!("statement failed:\n{statement}\n\n{e}"));
            executed += 1;
        }
        executed
    })
}
