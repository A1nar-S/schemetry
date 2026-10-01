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
use schemetry_lib::repositories::db_repository::LobCell;
use schemetry_lib::repositories::postgres_repository::DbPostgresRepository;
use schemetry_lib::services::cancel;
use schemetry_lib::services::compare::compare_tables_across_servers;
use schemetry_lib::services::fix::{generate_fix_script, Dialect};
use schemetry_lib::services::query::QueryService;
use schemetry_lib::services::schema_diff::SchemaDiffService;

use common::postgres as pg;

fn diff_service() -> SchemaDiffService {
    SchemaDiffService::new(Arc::new(DbPostgresRepository::new()))
}

fn dialects() -> HashMap<i64, Dialect> {
    let mut m = HashMap::new();
    m.insert(common::SOURCE_ID, Dialect::Postgres);
    m.insert(common::TARGET_ID, Dialect::Postgres);
    m
}

fn server_names() -> HashMap<i64, String> {
    [
        (common::SOURCE_ID, "SOURCE".to_string()),
        (common::TARGET_ID, "TARGET".to_string()),
    ]
    .into_iter()
    .collect()
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
        compare_tables_across_servers(&servers, &server_names(), common::SOURCE_ID, true, true, true, true)
            .expect("compare failed");

    let has = |diff: &str, element: &str, table: &str, column: &str| {
        discrepancies.iter().any(|d| {
            d.difference == diff
                && d.element == element
                && d.table_name.eq_ignore_ascii_case(table)
                && d.column_name.eq_ignore_ascii_case(column)
                && d.server_id == common::TARGET_ID
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
    server_table_ddls.insert(common::SOURCE_ID, ddls);

    let fix = generate_fix_script(
        &discrepancies,
        &selected,
        &servers,
        &server_table_ddls,
        common::SOURCE_ID,
        &server_names(),
        &dialects(),
    )
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
    let discrepancies_after =
        compare_tables_across_servers(&servers_after, &server_names(), common::SOURCE_ID, true, true, true, true)
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
    let discrepancies_final =
        compare_tables_across_servers(&servers_final, &server_names(), common::SOURCE_ID, true, true, true, true)
            .expect("compare after second fix run failed");

    assert_eq!(
        discrepancies_after.len(),
        discrepancies_final.len(),
        "re-running the fix script should not change the discrepancy count\nafter first run: {discrepancies_after:?}\nafter second run: {discrepancies_final:?}"
    );
}

// ── LOB cells (`lob_documents`, seeded by the V2 migrations) ─────────────────────
// Only `bytea` is a LOB here.

const LOB_QUERY: &str = "SELECT doc_id, title, body, payload FROM lob_documents ORDER BY doc_id";
const PNG_BYTES: [u8; 12] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

fn lob_bytes(cell: LobCell) -> Vec<u8> {
    match cell {
        LobCell::Binary(bytes) => bytes,
        other => panic!("expected a binary cell, got {other:?}"),
    }
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn lob_cells_are_read_from_the_kept_result() {
    let svc = QueryService::new(Arc::new(DbPostgresRepository::new()));

    let results = svc.run_query_on_servers(&[pg::source_connection()], LOB_QUERY, false);
    let r = &results[0];
    assert!(r.error.is_none(), "unexpected query error: {:?}", r.error);
    assert_eq!(r.column_types[2..], ["TEXT", "BYTEA"]);
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[0][2].as_deref(), Some("Price: 5 € — ✓ done"));
    assert_eq!(r.rows[0][3].as_deref(), Some("<BYTEA>"));
    assert_eq!(r.rows[1][3], None);

    let read = |row, max_bytes| {
        svc.read_lob_cell(r.result_id, row, 3, max_bytes, usize::MAX)
            .expect("read_lob_cell failed")
    };
    assert_eq!(lob_bytes(read(0, usize::MAX)), PNG_BYTES);
    assert!(lob_bytes(read(1, usize::MAX)).is_empty());

    let expected = common::large_lob_content();
    assert_eq!(lob_bytes(read(2, 1_500_000)), expected[..1_500_000]);
    let mut full = Vec::new();
    let size = svc.copy_lob_cell(r.result_id, 2, 3, &mut full).expect("copy_lob_cell failed");
    assert_eq!(size, 3_000_000);
    assert!(full == expected, "copied content differs from the seeded row");

    assert!(
        svc.read_lob_cell(r.result_id, 0, 2, usize::MAX, usize::MAX).is_err(),
        "a text column is not a LOB cell"
    );
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn lob_cells_materialize_inline_when_requested() {
    let svc = QueryService::new(Arc::new(DbPostgresRepository::new()));

    let results = svc.run_query_on_servers(&[pg::source_connection()], LOB_QUERY, true);
    let r = &results[0];
    assert!(r.error.is_none(), "unexpected query error: {:?}", r.error);
    assert_eq!(r.rows[0][3].as_deref(), Some("89504E470D0A1A0A00000000"));

    let mut full = Vec::new();
    svc.copy_lob_cell(r.result_id, 2, 3, &mut full).expect("copy_lob_cell failed");
    assert_eq!(full.len(), 3_000_000);
}

/// LOB cells reflect the data as of the query and are released by the next run.
#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml; writes to TARGET"]
fn lob_result_is_a_snapshot_released_by_the_next_run() {
    let svc = QueryService::new(Arc::new(DbPostgresRepository::new()));
    let target = pg::target_connection();

    pg::execute_script(
        &target,
        "DELETE FROM lob_documents WHERE doc_id = 900;\n\
         INSERT INTO lob_documents (doc_id, title, payload) VALUES (900, 'snapshot', decode('0102', 'hex'));",
    );

    let results = svc.run_query_on_servers(
        &[target.clone()],
        "SELECT payload FROM lob_documents WHERE doc_id = 900",
        false,
    );
    let result_id = results[0].result_id;
    assert!(results[0].error.is_none(), "unexpected query error: {:?}", results[0].error);

    pg::execute_script(&target, "UPDATE lob_documents SET payload = decode('0304', 'hex') WHERE doc_id = 900;");

    let payload = svc.read_lob_cell(result_id, 0, 0, usize::MAX, usize::MAX).unwrap();
    assert_eq!(lob_bytes(payload), [0x01, 0x02]);

    svc.run_query_on_servers(&[target.clone()], "SELECT 1 AS one", false);
    assert!(
        svc.read_lob_cell(result_id, 0, 0, usize::MAX, usize::MAX).is_err(),
        "the previous result should be released by the next run"
    );

    pg::execute_script(&target, "DELETE FROM lob_documents WHERE doc_id = 900;");
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn completion_metadata_lists_relations_and_columns() {
    let svc = QueryService::new(Arc::new(DbPostgresRepository::new()));

    let meta = svc.completion_metadata(&pg::source_connection()).expect("completion_metadata failed");
    assert_eq!(meta.schema, "public");
    let departments = meta.relations.iter().find(|r| r.name == "departments").expect("departments missing");
    assert_eq!(departments.kind, "TABLE");
    let columns: Vec<(&str, &str)> =
        departments.columns.iter().map(|c| (c.name.as_str(), c.data_type.as_str())).collect();
    assert_eq!(columns, [("dept_id", "integer"), ("dept_name", "character varying(50)")]);
    assert!(meta.relations.iter().any(|r| r.name == "lob_samples"));
    // Flyway's own history table lives in the schema too.
    assert!(meta.relations.iter().any(|r| r.name == "flyway_schema_history"));
}

#[test]
#[ignore = "requires the two Postgres containers from docker/docker-compose.yml"]
fn running_query_can_be_cancelled() {
    let svc = QueryService::new(Arc::new(DbPostgresRepository::new()));

    let task = cancel::begin_task();
    let stop = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        assert!(cancel::cancel_task(), "no task was running to cancel");
    });

    let started = std::time::Instant::now();
    let results = svc.run_query_on_servers(&[pg::source_connection()], "SELECT pg_sleep(60)", false);
    stop.join().unwrap();

    assert_eq!(results[0].error.as_deref(), Some("Cancelled by user."));
    assert!(started.elapsed().as_secs() < 30, "cancel took {:?}", started.elapsed());
    assert!(task.is_cancelled());
    drop(task);
    assert!(!cancel::cancel_task(), "task should be cleared once its guard drops");
}
