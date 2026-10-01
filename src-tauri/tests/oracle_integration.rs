//! End-to-end tests against two real Oracle instances started by
//! `docker/docker-compose.yml` and migrated by Flyway (`docker/migrations/*`).
//!
//! These are separate from the unit tests under `src-tauri/src/**/tests/` (which run
//! as part of plain `cargo test`): they need a live Oracle Instant Client plus the two
//! containers, so every test here is `#[ignore]`d. Run them explicitly once the
//! containers are up and healthy:
//!
//!   cd docker && docker compose up -d --build
//!   cd .. && cargo test --test oracle_integration -- --ignored
//!
//! Connection details default to the docker-compose file's ports/credentials and can
//! be overridden with `SCHEMETRY_TEST_ORACLE_*` env vars (see `tests/common/mod.rs`).
//! If Instant Client isn't already on `PATH`, set `ORACLE_CLIENT_LIB_DIR`.

mod common;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use schemetry_lib::models::ServerTableDdls;
use schemetry_lib::repositories::db_repository::LobCell;
use schemetry_lib::repositories::oracle_repository::DbOracleRepository;
use schemetry_lib::services::cancel;
use schemetry_lib::services::compare::compare_tables_across_servers;
use schemetry_lib::services::fix::generate_fix_script;
use schemetry_lib::services::query::QueryService;
use schemetry_lib::services::schema_diff::SchemaDiffService;

fn diff_service() -> SchemaDiffService {
    SchemaDiffService::new(Arc::new(DbOracleRepository::new()))
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn connections_to_both_servers_succeed() {
    common::init_oracle_client();
    let svc = diff_service();

    svc.test_connection(&common::source_connection())
        .expect("connect to SOURCE failed - is `docker compose up` running and healthy?");
    svc.test_connection(&common::target_connection())
        .expect("connect to TARGET failed - is `docker compose up` running and healthy?");
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn ddl_and_schema_objects_on_source() {
    common::init_oracle_client();
    let svc = diff_service();
    let source = common::source_connection();

    let objects = svc
        .fetch_schema_objects(&source, &[])
        .expect("fetch_schema_objects failed");
    for expected in ["DEPARTMENTS", "EMPLOYEES", "AUDIT_LOG"] {
        assert!(
            objects
                .iter()
                .any(|o| o.name.eq_ignore_ascii_case(expected) && o.object_type == "TABLE"),
            "expected table {expected} among schema objects, got: {objects:?}"
        );
    }

    let ddl = svc
        .fetch_object_ddl(&source, "DEPARTMENTS", "TABLE")
        .expect("fetch_object_ddl failed");
    assert!(
        ddl.to_uppercase().contains("DEPARTMENTS"),
        "DDL didn't mention the table name: {ddl}"
    );
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn multi_server_query_execution() {
    common::init_oracle_client();
    let query_svc = QueryService::new(Arc::new(DbOracleRepository::new()));
    let connections = vec![common::source_connection(), common::target_connection()];

    let results =
        query_svc.run_query_on_servers(&connections, "SELECT COUNT(*) AS CNT FROM DEPARTMENTS", false);

    assert_eq!(results.len(), 2);
    for result in &results {
        assert!(
            result.error.is_none(),
            "{}: unexpected query error: {:?}",
            result.server_name,
            result.error
        );
        assert_eq!(result.columns, vec!["CNT".to_string()]);
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
/// Then re-run the same script to prove it's idempotent (every generated statement
/// guards itself with an existence/definition check before executing).
///
/// This test mutates TARGET's schema (adds AUDIT_LOG and EMPLOYEES.EMAIL, widens
/// EMPLOYEES.FIRST_NAME) - the other tests in this file never touch those objects, so
/// running everything in parallel (the default) is safe.
#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml; mutates TARGET"]
fn schema_diff_and_idempotent_fix_execution() {
    common::init_oracle_client();
    let diff_svc = diff_service();
    let source = common::source_connection();
    let target = common::target_connection();

    let server_names: HashMap<i64, String> = [
        (common::SOURCE_ID, "SOURCE".to_string()),
        (common::TARGET_ID, "TARGET".to_string()),
    ]
    .into_iter()
    .collect();

    let (servers, errors) = diff_svc.fetch_from_connections(&[source.clone(), target.clone()], &[]);
    assert!(errors.is_empty(), "fetch errors: {errors:?}");

    let discrepancies =
        compare_tables_across_servers(&servers, &server_names, common::SOURCE_ID, true, true, true, true)
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
        has("MISSING", "TABLE", "AUDIT_LOG", ""),
        "expected AUDIT_LOG missing on TARGET: {discrepancies:?}"
    );
    assert!(
        has("MISSING", "COLUMN", "EMPLOYEES", "EMAIL"),
        "expected EMPLOYEES.EMAIL missing on TARGET: {discrepancies:?}"
    );
    assert!(
        has("DIFFERENT", "DATA_LENGTH", "EMPLOYEES", "FIRST_NAME"),
        "expected EMPLOYEES.FIRST_NAME length mismatch on TARGET: {discrepancies:?}"
    );

    let selected: HashSet<usize> = (0..discrepancies.len()).collect();
    let ddls = diff_svc
        .fetch_table_ddls_for_tables(&source, &["AUDIT_LOG".to_string()])
        .expect("fetch_table_ddls_for_tables failed");
    let mut server_table_ddls: ServerTableDdls = HashMap::new();
    server_table_ddls.insert(common::SOURCE_ID, ddls);

    let server_dialects = HashMap::new();
    let fix = generate_fix_script(
        &discrepancies,
        &selected,
        &servers,
        &server_table_ddls,
        common::SOURCE_ID,
        &server_names,
        &server_dialects,
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

    let target_db = common::raw_connect(&target);
    let executed_first =
        common::execute_script(&target_db, &fix.script).expect("first fix execution failed");
    assert!(executed_first > 0, "expected at least one statement to execute");

    let expected_resolved = [
        ("MISSING", "TABLE", "AUDIT_LOG", ""),
        ("MISSING", "COLUMN", "EMPLOYEES", "EMAIL"),
        ("DIFFERENT", "DATA_LENGTH", "EMPLOYEES", "FIRST_NAME"),
    ];

    let (servers_after, errors_after) =
        diff_svc.fetch_from_connections(&[source.clone(), target.clone()], &[]);
    assert!(errors_after.is_empty(), "fetch errors after fix: {errors_after:?}");
    let discrepancies_after =
        compare_tables_across_servers(&servers_after, &server_names, common::SOURCE_ID, true, true, true, true)
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
    // every generated block checks USER_TABLES/USER_TAB_COLUMNS before acting.
    let executed_second =
        common::execute_script(&target_db, &fix.script).expect("second (idempotent) fix execution failed");
    assert!(executed_second > 0);

    let (servers_final, errors_final) = diff_svc.fetch_from_connections(&[source, target], &[]);
    assert!(errors_final.is_empty(), "fetch errors after second run: {errors_final:?}");
    let discrepancies_final =
        compare_tables_across_servers(&servers_final, &server_names, common::SOURCE_ID, true, true, true, true)
            .expect("compare after second fix run failed");

    assert_eq!(
        discrepancies_after.len(),
        discrepancies_final.len(),
        "re-running the fix script should not change the discrepancy count\nafter first run: {discrepancies_after:?}\nafter second run: {discrepancies_final:?}"
    );
}

// ── LOB cells (`LOB_DOCUMENTS`, seeded by the V2 migrations) ─────────────────────

const LOB_QUERY: &str = "SELECT DOC_ID, TITLE, BODY, PAYLOAD FROM LOB_DOCUMENTS ORDER BY DOC_ID";
const PNG_BYTES: [u8; 12] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];

fn lob_text(cell: LobCell) -> Option<String> {
    match cell {
        LobCell::Text(text) => text,
        other => panic!("expected a text cell, got {other:?}"),
    }
}

fn lob_bytes(cell: LobCell) -> Vec<u8> {
    match cell {
        LobCell::Binary(bytes) => bytes,
        other => panic!("expected a binary cell, got {other:?}"),
    }
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn lob_cells_are_read_from_the_kept_result() {
    common::init_oracle_client();
    let svc = QueryService::new(Arc::new(DbOracleRepository::new()));

    let results = svc.run_query_on_servers(&[common::source_connection()], LOB_QUERY, false);
    let r = &results[0];
    assert!(r.error.is_none(), "unexpected query error: {:?}", r.error);
    assert_eq!(r.column_types[2..], ["CLOB", "BLOB"]);
    assert_eq!(r.rows.len(), 3);
    assert_eq!(r.rows[0][2].as_deref(), Some("<CLOB>"));
    assert_eq!(r.rows[0][3].as_deref(), Some("<BLOB>"));
    assert_eq!(r.rows[1][2], None);
    assert_eq!(r.rows[1][3], None);

    let read = |row, col, max_bytes, max_chars| {
        svc.read_lob_cell(r.result_id, row, col, max_bytes, max_chars)
            .expect("read_lob_cell failed")
    };
    assert_eq!(lob_text(read(0, 2, usize::MAX, usize::MAX)).as_deref(), Some("Price: 5 € — ✓ done"));
    assert_eq!(lob_bytes(read(0, 3, usize::MAX, usize::MAX)), PNG_BYTES);
    assert_eq!(lob_text(read(1, 2, usize::MAX, usize::MAX)), None);
    assert!(lob_bytes(read(1, 3, usize::MAX, usize::MAX)).is_empty());

    // Row 3 spans several read chunks.
    let expected = common::large_lob_content();
    let capped_text = lob_text(read(2, 2, usize::MAX, 1_500_000)).expect("row 3 body is not NULL");
    assert_eq!(capped_text.as_bytes(), &expected[..1_500_000]);
    assert_eq!(lob_bytes(read(2, 3, 1_500_000, usize::MAX)), expected[..1_500_000]);

    for col in [2, 3] {
        let mut full = Vec::new();
        let size = svc.copy_lob_cell(r.result_id, 2, col, &mut full).expect("copy_lob_cell failed");
        assert_eq!(size, 3_000_000);
        assert!(full == expected, "column {col}: copied content differs from the seeded row");
    }
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn lob_cells_materialize_inline_when_requested() {
    common::init_oracle_client();
    let svc = QueryService::new(Arc::new(DbOracleRepository::new()));

    let results = svc.run_query_on_servers(&[common::source_connection()], LOB_QUERY, true);
    let r = &results[0];
    assert!(r.error.is_none(), "unexpected query error: {:?}", r.error);
    assert_eq!(r.rows[0][2].as_deref(), Some("Price: 5 € — ✓ done"));
    assert_eq!(r.rows[0][3].as_deref(), Some("89504E470D0A1A0A00000000"));
    // Capped at 1,000,000 chars; the all-digit BLOB displays as text.
    assert_eq!(r.rows[2][2].as_ref().map(|s| s.chars().count()), Some(1_000_000));
    assert_eq!(r.rows[2][3].as_ref().map(|s| s.chars().count()), Some(1_000_000));

    let mut full = Vec::new();
    svc.copy_lob_cell(r.result_id, 2, 3, &mut full).expect("copy_lob_cell failed");
    assert_eq!(full.len(), 3_000_000);
}

/// LOB cells reflect the data as of the query and are released by the next run.
#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml; writes to TARGET"]
fn lob_result_is_a_snapshot_released_by_the_next_run() {
    common::init_oracle_client();
    let svc = QueryService::new(Arc::new(DbOracleRepository::new()));
    let target = common::target_connection();
    let db = common::raw_connect(&target);

    db.execute("DELETE FROM LOB_DOCUMENTS WHERE DOC_ID = 900", &[]).unwrap();
    db.execute(
        "INSERT INTO LOB_DOCUMENTS (DOC_ID, TITLE, BODY, PAYLOAD) \
         VALUES (900, 'snapshot', TO_CLOB('before'), HEXTORAW('0102'))",
        &[],
    )
    .unwrap();
    db.commit().unwrap();

    let results = svc.run_query_on_servers(
        &[target.clone()],
        "SELECT BODY, PAYLOAD FROM LOB_DOCUMENTS WHERE DOC_ID = 900",
        false,
    );
    let result_id = results[0].result_id;
    assert!(results[0].error.is_none(), "unexpected query error: {:?}", results[0].error);

    db.execute(
        "UPDATE LOB_DOCUMENTS SET BODY = TO_CLOB('after'), PAYLOAD = HEXTORAW('0304') WHERE DOC_ID = 900",
        &[],
    )
    .unwrap();
    db.commit().unwrap();

    let body = svc.read_lob_cell(result_id, 0, 0, usize::MAX, usize::MAX).unwrap();
    let payload = svc.read_lob_cell(result_id, 0, 1, usize::MAX, usize::MAX).unwrap();
    assert_eq!(lob_text(body).as_deref(), Some("before"));
    assert_eq!(lob_bytes(payload), [0x01, 0x02]);

    svc.run_query_on_servers(&[target], "SELECT 1 AS ONE FROM dual", false);
    assert!(
        svc.read_lob_cell(result_id, 0, 0, usize::MAX, usize::MAX).is_err(),
        "the previous result should be released by the next run"
    );

    db.execute("DELETE FROM LOB_DOCUMENTS WHERE DOC_ID = 900", &[]).unwrap();
    db.commit().unwrap();
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn completion_metadata_lists_relations_and_columns() {
    common::init_oracle_client();
    let svc = QueryService::new(Arc::new(DbOracleRepository::new()));

    let meta = svc.completion_metadata(&common::source_connection()).expect("completion_metadata failed");
    assert_eq!(meta.schema, "SCHEMETRY");
    let departments = meta.relations.iter().find(|r| r.name == "DEPARTMENTS").expect("DEPARTMENTS missing");
    assert_eq!(departments.kind, "TABLE");
    let columns: Vec<(&str, &str)> =
        departments.columns.iter().map(|c| (c.name.as_str(), c.data_type.as_str())).collect();
    assert_eq!(columns, [("DEPT_ID", "NUMBER"), ("DEPT_NAME", "VARCHAR2")]);
    assert!(meta.relations.iter().any(|r| r.name == "LOB_SAMPLES"));
}

#[test]
#[ignore = "requires the two Oracle containers from docker/docker-compose.yml"]
fn running_query_can_be_cancelled() {
    common::init_oracle_client();
    let svc = QueryService::new(Arc::new(DbOracleRepository::new()));

    let task = cancel::begin_task();
    let stop = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(cancel::cancel_task(), "no task was running to cancel");
    });

    let started = std::time::Instant::now();
    let results = svc.run_query_on_servers(
        &[common::source_connection()],
        "SELECT COUNT(*) FROM all_objects a, all_objects b, all_objects c",
        false,
    );
    stop.join().unwrap();

    assert_eq!(results[0].error.as_deref(), Some("Cancelled by user."));
    assert!(started.elapsed().as_secs() < 30, "cancel took {:?}", started.elapsed());
    assert!(task.is_cancelled());
    drop(task);
    assert!(!cancel::cancel_task(), "task should be cleared once its guard drops");
}
