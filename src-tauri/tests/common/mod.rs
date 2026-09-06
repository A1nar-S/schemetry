//! Shared setup for `oracle_integration.rs`: Oracle Instant Client init, connection
//! details for the two docker-compose containers, and a couple of raw-SQL helpers.
//!
//! Connection details default to `docker/docker-compose.yml`'s ports/credentials and
//! can be overridden with `SCHEMETRY_TEST_ORACLE_HOST`, `SCHEMETRY_TEST_SOURCE_PORT`,
//! `SCHEMETRY_TEST_TARGET_PORT`, `SCHEMETRY_TEST_ORACLE_SERVICE`,
//! `SCHEMETRY_TEST_ORACLE_USER`, `SCHEMETRY_TEST_ORACLE_PASSWORD`. If Instant Client
//! isn't already on `PATH`, set `ORACLE_CLIENT_LIB_DIR`.

pub mod postgres;

use std::env;

use oracle::Connection;
use schemetry_lib::models::{ConnectionRecord, DbType};
use schemetry_lib::repositories::oracle_repository::configure_client_lib_dir;

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

pub fn init_oracle_client() {
    if let Ok(dir) = env::var("ORACLE_CLIENT_LIB_DIR") {
        configure_client_lib_dir(&dir).expect("failed to initialize Oracle Instant Client");
    }
}

fn connection(name: &str, default_port: u16, port_env: &str) -> ConnectionRecord {
    let host = env_or("SCHEMETRY_TEST_ORACLE_HOST", "localhost");
    let port: u16 = env::var(port_env)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_port);
    let service_name = env_or("SCHEMETRY_TEST_ORACLE_SERVICE", "FREEPDB1");
    let username = env_or("SCHEMETRY_TEST_ORACLE_USER", "SCHEMETRY");
    let password = env_or("SCHEMETRY_TEST_ORACLE_PASSWORD", "SchemetryTest_2024");

    ConnectionRecord {
        id: 0,
        name: name.to_string(),
        db_type: DbType::Oracle,
        host,
        port,
        service_name,
        username,
        password,
        group_name: "Integration Tests".to_string(),
        pg_schema: String::new(),
        password_broken: false,
    }
}

pub fn source_connection() -> ConnectionRecord {
    connection("SOURCE", 1521, "SCHEMETRY_TEST_SOURCE_PORT")
}

pub fn target_connection() -> ConnectionRecord {
    connection("TARGET", 1522, "SCHEMETRY_TEST_TARGET_PORT")
}

/// Opens a raw connection for helpers (executing the generated fix script) that don't
/// go through `OracleRepository`.
pub fn raw_connect(conn: &ConnectionRecord) -> Connection {
    let connect_string = format!("//{}:{}/{}", conn.host, conn.port, conn.service_name);
    Connection::connect(&conn.username, &conn.password, &connect_string)
        .expect("raw connection failed")
}

/// Executes a `;`-separated fix script, statement by statement, and returns how many
/// ran. Blank statements (trailing separator, comments-only) are skipped.
pub fn execute_script(db: &Connection, script: &str) -> Result<usize, oracle::Error> {
    let mut executed = 0;
    for statement in script.split(';') {
        let trimmed = statement.trim();
        if trimmed.is_empty() {
            continue;
        }
        db.execute(trimmed, &[])?;
        executed += 1;
    }
    db.commit()?;
    Ok(executed)
}
