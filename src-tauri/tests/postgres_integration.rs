//! End-to-end tests against two real Postgres instances started by
//! `docker/docker-compose.yml` and migrated by Flyway
//! (`docker/migrations/pg-source`, `docker/migrations/pg-target`).
//!
//! Mirrors `oracle_integration.rs` — see that file for the general shape. These are
//! separate from the unit tests under `src-tauri/src/**/tests/` (which run as part of
//! plain `cargo test`): they need the two Postgres containers, so every test here is
//! `#[ignore]`d. Run them explicitly once the containers are up and healthy:
//!
//!   cd docker && docker compose up -d --build
//!   cd .. && cargo test --test postgres_integration -- --ignored
//!
//! Connection details default to the docker-compose file's ports/credentials and can
//! be overridden with `SCHEMETRY_TEST_PG_*` env vars (see `tests/common/postgres.rs`).
//! Unlike the Oracle suite, no native client library setup is needed —
//! `tokio-postgres` is a pure-Rust wire-protocol client.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use schemetry_lib::models::ServerTableDdls;
use schemetry_lib::repositories::postgres_repository::DbPostgresRepository;
use schemetry_lib::services::compare::compare_tables_across_servers;
use schemetry_lib::services::fix::{generate_fix_script, Dialect};
use schemetry_lib::services::query::QueryService;
use schemetry_lib::services::schema_diff::SchemaDiffService;

use common::postgres as pg;

fn diff_service() -> SchemaDiffService {
    SchemaDiffService::new(Arc::new(DbPostgresRepository::new()))
}

fn dialects() -> HashMap<String, Dialect> {
    let mut m = HashMap::new();
    m.insert("SOURCE".to_string(), Dialect::Postgres);
    m.insert("TARGET".to_string(), Dialect::Postgres);
    m
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn connections_to_both_servers_succeed() {
    let svc = diff_service();

    svc.test_connection(&pg::source_connection())
        .expect("connect to SOURCE failed - is `docker compose up` running and healthy?");
    svc.test_connection(&pg::target_connection())
        .expect("connect to TARGET failed - is `docker compose up` running and healthy?");
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn ddl_and_schema_objects_on_source() {
    let svc = diff_service();
    let source = pg::source_connection();

    let objects = svc
        .fetch_schema_objects(&source, &[])
        .expect("fetch_schema_objects failed");
    for expected in ["departments", "employees", "audit_log"] {
        assert!(
            objects
                .iter()
                .any(|o| o.name.eq_ignore_ascii_case(expected) && o.object_type == "TABLE"),
            "expected table {expected} among schema objects, got: {objects:?}"
        );
    }

    let ddl = svc
        .fetch_object_ddl(&source, "departments", "TABLE")
        .expect("fetch_object_ddl failed");
    assert!(
        ddl.to_lowercase().contains("departments"),
        "DDL didn't mention the table name: {ddl}"
    );
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn multi_server_query_execution() {
    let query_svc = QueryService::new(Arc::new(DbPostgresRepository::new()));
    let connections = vec![pg::source_connection(), pg::target_connection()];

    let results =
        query_svc.run_query_on_servers(&connections, "SELECT COUNT(*) AS cnt FROM departments", false);

    assert_eq!(results.len(), 2);
    for result in &results {
        assert!(
            result.error.is_none(),
            "{}: unexpected query error: {:?}",
            result.server_name,
            result.error
        );
        assert_eq!(result.columns, vec!["cnt".to_string()]);
        assert_eq!(result.rows.len(), 1, "expected exactly one row from {}", result.server_name);
        assert_eq!(
            result.rows[0][0].as_deref(),
            Some("2"),
            "{}: both migrations seed 2 departments",
            result.server_name
        );
    }
}

/// The main scenario: fetch both schemas, confirm the discrepancies the divergent
/// migrations were designed to produce, generate a fix script against SOURCE as the
/// reference, execute it against TARGET, and confirm those discrepancies are gone.
/// Then re-run the same script to prove it's idempotent (native `IF NOT EXISTS`/
/// `DO $$ ... $$` guards, same idea as the Oracle suite's guarded PL/SQL blocks).
///
/// Unlike Oracle, a Postgres length-only difference (`first_name`'s `varchar(30)` vs.
/// `varchar(50)`) shows up as a `DATA_TYPE` discrepancy, not `DATA_LENGTH` — the
/// Postgres repository stores the whole `format_type()` output (e.g.
/// `character varying(30)`) as `data_type` and always leaves `data_length` unset.
///
/// This test mutates TARGET's schema — the other tests in this file never touch those
/// objects, so running everything in parallel (the default) is safe.
#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml; mutates TARGET"]
fn schema_diff_and_idempotent_fix_execution() {
    let diff_svc = diff_service();
    let source = pg::source_connection();
    let target = pg::target_connection();

    let (servers, errors) = diff_svc.fetch_from_connections(&[source.clone(), target.clone()], &[]);
    assert!(errors.is_empty(), "fetch errors: {errors:?}");

    let discrepancies =
        compare_tables_across_servers(&servers, "SOURCE", true, true).expect("compare failed");

    let has = |diff: &str, element: &str, table: &str, column: &str| {
        discrepancies.iter().any(|d| {
            d.difference == diff
                && d.element == element
                && d.table_name.eq_ignore_ascii_case(table)
                && d.column_name.eq_ignore_ascii_case(column)
                && d.server_name.eq_ignore_ascii_case("TARGET")
        })
    };

    assert!(
        has("MISSING", "TABLE", "audit_log", ""),
        "expected audit_log missing on TARGET: {discrepancies:?}"
    );
    assert!(
        has("MISSING", "COLUMN", "employees", "email"),
        "expected employees.email missing on TARGET: {discrepancies:?}"
    );
    assert!(
        has("DIFFERENT", "DATA_TYPE", "employees", "first_name"),
        "expected employees.first_name type mismatch on TARGET: {discrepancies:?}"
    );

    let selected: HashSet<usize> = (0..discrepancies.len()).collect();
    let ddls = diff_svc
        .fetch_table_ddls_for_tables(&source, &["audit_log".to_string()])
        .expect("fetch_table_ddls_for_tables failed");
    let mut server_table_ddls: ServerTableDdls = HashMap::new();
    server_table_ddls.insert("SOURCE".to_string(), ddls);

    let fix = generate_fix_script(&discrepancies, &selected, &servers, &server_table_ddls, "SOURCE", &dialects())
        .expect("generate_fix_script failed");
    assert_eq!(
        fix.generated_count, 3,
        "expected exactly 3 generated statements, script:\n{}",
        fix.script
    );
    assert_eq!(
        fix.skipped_count, 0,
        "expected nothing skipped, script:\n{}",
        fix.script
    );

    let executed_first = pg::execute_script(&target, &fix.script);
    assert!(executed_first > 0, "expected at least one statement to execute");

    let expected_resolved = [
        ("MISSING", "TABLE", "audit_log", ""),
        ("MISSING", "COLUMN", "employees", "email"),
        ("DIFFERENT", "DATA_TYPE", "employees", "first_name"),
    ];

    let (servers_after, errors_after) =
        diff_svc.fetch_from_connections(&[source.clone(), target.clone()], &[]);
    assert!(errors_after.is_empty(), "fetch errors after fix: {errors_after:?}");
    let discrepancies_after = compare_tables_across_servers(&servers_after, "SOURCE", true, true)
        .expect("compare after fix failed");

    for (diff, element, table, column) in expected_resolved {
        assert!(
            !discrepancies_after.iter().any(|d| d.difference == diff
                && d.element == element
                && d.table_name.eq_ignore_ascii_case(table)
                && d.column_name.eq_ignore_ascii_case(column)),
            "discrepancy {diff}/{element}/{table}.{column} still present after fix: {discrepancies_after:?}"
        );
    }

    // Idempotency: re-running the identical script must succeed and change nothing -
    // every generated statement is guarded with `IF NOT EXISTS`/a `DO $$ ... $$` check.
    let executed_second = pg::execute_script(&target, &fix.script);
    assert!(executed_second > 0);

    let (servers_final, errors_final) = diff_svc.fetch_from_connections(&[source, target], &[]);
    assert!(errors_final.is_empty(), "fetch errors after second run: {errors_final:?}");
    let discrepancies_final = compare_tables_across_servers(&servers_final, "SOURCE", true, true)
        .expect("compare after second fix run failed");

    assert_eq!(
        discrepancies_after.len(),
        discrepancies_final.len(),
        "re-running the fix script should not change the discrepancy count\nafter first run: {discrepancies_after:?}\nafter second run: {discrepancies_final:?}"
    );
}
