use super::*;
use std::collections::{HashMap, HashSet};
use crate::models::{ColumnInfo, Discrepancy, TableColumns};

fn disc(difference: &str, element: &str, table: &str, column: &str, server: &str, details: &str) -> Discrepancy {
    Discrepancy {
        difference: difference.to_string(),
        element: element.to_string(),
        table_name: table.to_string(),
        column_name: column.to_string(),
        server_name: server.to_string(),
        details: details.to_string(),
    }
}

fn col_typed(data_type: &str) -> ColumnInfo {
    ColumnInfo { data_type: Some(data_type.to_string()), ..Default::default() }
}

fn col_typed_len(data_type: &str, length: &str) -> ColumnInfo {
    ColumnInfo {
        data_type: Some(data_type.to_string()),
        data_length: Some(length.to_string()),
        ..Default::default()
    }
}

fn make_servers(server: &str, table: &str, column: &str, info: ColumnInfo) -> ServersData {
    let mut col_map: TableColumns = HashMap::new();
    col_map.insert(column.to_string(), info);
    let mut table_map = HashMap::new();
    table_map.insert(table.to_string(), col_map);
    let mut servers: ServersData = HashMap::new();
    servers.insert(server.to_string(), table_map);
    servers
}

fn ids(xs: &[usize]) -> HashSet<usize> {
    xs.iter().copied().collect()
}

// ── sql_literal ───────────────────────────────────────────────────

#[test]
fn sql_literal_plain() {
    assert_eq!(sql_literal("hello"), "'hello'");
}

#[test]
fn sql_literal_escapes_single_quotes() {
    assert_eq!(sql_literal("it's"), "'it''s'");
}

// ── quoted_ident ──────────────────────────────────────────────────

#[test]
fn quoted_ident_plain() {
    assert_eq!(quoted_ident("MY_TABLE"), "\"MY_TABLE\"");
}

#[test]
fn quoted_ident_escapes_double_quote() {
    assert_eq!(quoted_ident("A\"B"), "\"A\"\"B\"");
}

// ── parse_target_server_from_details ──────────────────────────────

#[test]
fn parse_target_server_valid() {
    let details = "Column foo found in server PROD_DB but not in reference server DEV_DB";
    assert_eq!(
        parse_target_server_from_details(details),
        Some("PROD_DB".to_string())
    );
}

#[test]
fn parse_target_server_no_marker_returns_none() {
    assert_eq!(parse_target_server_from_details("random text"), None);
}

// ── detect_fix_kind ───────────────────────────────────────────────

#[test]
fn detect_fix_kind_missing_table() {
    assert_eq!(detect_fix_kind(&disc("MISSING", "TABLE", "T", "", "S", "...")), FixKind::MissingTable);
}

#[test]
fn detect_fix_kind_missing_column() {
    assert_eq!(detect_fix_kind(&disc("MISSING", "COLUMN", "T", "C", "S", "...")), FixKind::MissingColumn);
}

#[test]
fn detect_fix_kind_data_type() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "DATA_TYPE", "T", "C", "S", "DATA_TYPE: NUMBER != VARCHAR2")), FixKind::DataType);
}

#[test]
fn detect_fix_kind_data_default() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "DATA_DEFAULT", "T", "C", "S", "DATA_DEFAULT: 0 != NULL")), FixKind::DataDefault);
}

#[test]
fn detect_fix_kind_comments() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "COMMENTS", "T", "C", "S", "COMMENTS: Foo != Bar")), FixKind::Comments);
}

#[test]
fn detect_fix_kind_index_name() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "INDEX_NAME", "T", "C", "S", "INDEX_NAME: IDX_A != IDX_B")), FixKind::IndexName);
}

#[test]
fn detect_fix_kind_data_length() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "DATA_LENGTH", "T", "C", "S", "VARCHAR2(50) != VARCHAR2(100)")), FixKind::DataLength);
}

#[test]
fn detect_fix_kind_unsupported() {
    assert_eq!(detect_fix_kind(&disc("DIFFERENT", "UNKNOWN", "T", "C", "S", "something weird")), FixKind::Unsupported);
}

// ── resolve_target_server ─────────────────────────────────────────

#[test]
fn resolve_target_server_non_reference() {
    let d = disc("DIFFERENT", "DATA_TYPE", "T", "C", "TARGET_SRV", "...");
    assert_eq!(resolve_target_server(&d, "REF_SRV"), Some("TARGET_SRV".to_string()));
}

#[test]
fn resolve_target_server_reference_with_missing() {
    let d = disc("MISSING", "COLUMN", "T", "C", "REF_SRV",
        "Column foo found in server OTHER_SRV but not in reference server REF_SRV");
    assert_eq!(resolve_target_server(&d, "REF_SRV"), Some("OTHER_SRV".to_string()));
}

#[test]
fn resolve_target_server_reference_with_different_returns_none() {
    let d = disc("DIFFERENT", "DATA_TYPE", "T", "C", "REF_SRV", "DATA_TYPE: A != B");
    assert_eq!(resolve_target_server(&d, "REF_SRV"), None);
}

// ── normalize_ddl_for_execute_immediate ───────────────────────────

#[test]
fn normalize_empty_returns_none() {
    assert!(normalize_ddl_for_execute_immediate("").is_none());
    assert!(normalize_ddl_for_execute_immediate("   ").is_none());
}

#[test]
fn normalize_strips_trailing_semicolon() {
    assert_eq!(
        normalize_ddl_for_execute_immediate("CREATE TABLE T (ID NUMBER);"),
        Some("CREATE TABLE T (ID NUMBER)".to_string())
    );
}

#[test]
fn normalize_strips_trailing_slash() {
    assert_eq!(
        normalize_ddl_for_execute_immediate("CREATE TABLE T (ID NUMBER)/"),
        Some("CREATE TABLE T (ID NUMBER)".to_string())
    );
}

#[test]
fn normalize_strips_multiple_terminators() {
    assert_eq!(
        normalize_ddl_for_execute_immediate("CREATE TABLE T (ID NUMBER);/"),
        Some("CREATE TABLE T (ID NUMBER)".to_string())
    );
}

#[test]
fn normalize_preserves_interior_semicolons() {
    let ddl = "BEGIN\n  EXECUTE IMMEDIATE 'x';\nEND;";
    let result = normalize_ddl_for_execute_immediate(ddl).unwrap();
    assert!(result.contains("EXECUTE IMMEDIATE 'x';"));
    assert!(!result.ends_with(';'));
}

// ── build_create_table_block ──────────────────────────────────────

#[test]
fn build_create_table_block_produces_idempotent_plsql() {
    let ddl = "CREATE TABLE ORDERS (ID NUMBER)";
    let block = build_create_table_block("ORDERS", ddl).unwrap();
    assert!(block.contains("'ORDERS'"), "block: {block}");
    assert!(block.contains("USER_TABLES"), "block: {block}");
    assert!(block.contains("IF v_table_count = 0 THEN"), "block: {block}");
    assert!(block.contains("EXECUTE IMMEDIATE"), "block: {block}");
}

#[test]
fn build_create_table_block_empty_ddl_returns_none() {
    assert!(build_create_table_block("T", "").is_none());
}

// ── build_add_column_block ────────────────────────────────────────

#[test]
fn build_add_column_basic_varchar2() {
    let info = col_typed_len("VARCHAR2", "50");
    let block = build_add_column_block("MY_TABLE", "MY_COL", &info).unwrap();
    assert!(block.contains("ALTER TABLE"), "block: {block}");
    assert!(block.contains("ADD"), "block: {block}");
    assert!(block.contains("VARCHAR2(50)"), "block: {block}");
    assert!(block.contains("'MY_TABLE'"), "block: {block}");
    assert!(block.contains("'MY_COL'"), "block: {block}");
}

#[test]
fn build_add_column_with_comment_appends_comment_block() {
    let info = ColumnInfo {
        comments: Some("Primary key".to_string()),
        ..col_typed("NUMBER")
    };
    let block = build_add_column_block("T", "C", &info).unwrap();
    assert!(block.contains("COMMENT ON COLUMN"), "block: {block}");
    assert!(block.contains("Primary key"), "block: {block}");
}

#[test]
fn build_add_column_no_data_type_returns_none() {
    let info = ColumnInfo { data_type: None, ..Default::default() };
    assert!(build_add_column_block("T", "C", &info).is_none());
}

// ── build_modify_type_block ───────────────────────────────────────

#[test]
fn build_modify_type_block_checks_type_before_modify() {
    let info = col_typed_len("NUMBER", "10");
    let block = build_modify_type_block("T", "C", &info).unwrap();
    assert!(block.contains("MODIFY"), "block: {block}");
    assert!(block.contains("NUMBER(10)"), "block: {block}");
    assert!(block.contains("USER_TAB_COLUMNS"), "block: {block}");
}

// ── build_default_block ───────────────────────────────────────────

#[test]
fn build_default_block_with_default_uses_expr() {
    let info = ColumnInfo { data_default: Some("SYSDATE".to_string()), ..col_typed("DATE") };
    let block = build_default_block("T", "C", &info).unwrap();
    assert!(block.contains("DEFAULT SYSDATE"), "block: {block}");
}

#[test]
fn build_default_block_without_default_uses_null() {
    let block = build_default_block("T", "C", &col_typed("NUMBER")).unwrap();
    assert!(block.contains("DEFAULT NULL"), "block: {block}");
}

// ── build_comment_block ───────────────────────────────────────────

#[test]
fn build_comment_block_contains_comment_sql() {
    let info = ColumnInfo { comments: Some("The primary key".to_string()), ..col_typed("NUMBER") };
    let block = build_comment_block("T", "C", &info).unwrap();
    assert!(block.contains("COMMENT ON COLUMN"), "block: {block}");
    assert!(block.contains("The primary key"), "block: {block}");
}

// ── generate_fix_script ───────────────────────────────────────────

#[test]
fn generate_fix_empty_selection_returns_err() {
    let servers = make_servers("REF", "T", "C", col_typed("NUMBER"));
    let result = generate_fix_script(&[], &HashSet::new(), &servers, &HashMap::new(), "REF");
    assert!(result.is_err());
}

#[test]
fn generate_fix_missing_column_produces_add_statement() {
    let ref_col = col_typed_len("VARCHAR2", "100");
    let mut servers = make_servers("REF", "T", "COL1", ref_col);
    let mut tgt_cols: TableColumns = HashMap::new();
    tgt_cols.insert("OTHER".to_string(), col_typed("NUMBER")); // table exists, but without COL1
    let mut tgt_tables = HashMap::new();
    tgt_tables.insert("T".to_string(), tgt_cols);
    servers.insert("TGT".to_string(), tgt_tables);

    let discs = vec![disc(
        "MISSING", "COLUMN", "T", "COL1", "TGT",
        "Column found in reference server REF but not in server TGT",
    )];
    let result = generate_fix_script(&discs, &ids(&[0]), &servers, &HashMap::new(), "REF").unwrap();
    assert!(result.script.contains("ALTER TABLE"), "script: {}", result.script);
    assert!(result.script.contains("ADD"), "script: {}", result.script);
    assert_eq!(result.generated_count, 1);
    assert_eq!(result.skipped_count, 0);
}

#[test]
fn generate_fix_data_type_produces_modify_statement() {
    let ref_col = col_typed("NUMBER");
    let tgt_col = ColumnInfo { data_type: Some("VARCHAR2".to_string()), ..Default::default() };
    let mut servers = make_servers("REF", "T", "C", ref_col);
    let mut tgt_cols: TableColumns = HashMap::new();
    tgt_cols.insert("C".to_string(), tgt_col);
    let mut tgt_tables = HashMap::new();
    tgt_tables.insert("T".to_string(), tgt_cols);
    servers.insert("TGT".to_string(), tgt_tables);

    let discs = vec![disc("DIFFERENT", "DATA_TYPE", "T", "C", "TGT", "DATA_TYPE: NUMBER != VARCHAR2")];
    let result = generate_fix_script(&discs, &ids(&[0]), &servers, &HashMap::new(), "REF").unwrap();
    assert!(result.script.contains("MODIFY"), "script: {}", result.script);
    assert_eq!(result.generated_count, 1);
}

#[test]
fn generate_fix_column_name_discrepancy_is_skipped() {
    let mut servers: ServersData = HashMap::new();
    servers.insert("REF".to_string(), HashMap::new());

    let discs = vec![disc("DIFFERENT", "COLUMN_NAME", "T", "C", "TGT", "COLUMN_NAME: OLD != NEW")];
    let result = generate_fix_script(&discs, &ids(&[0]), &servers, &HashMap::new(), "REF").unwrap();
    assert_eq!(result.skipped_count, 1);
    assert!(result.script.contains("-- Skipped"), "script: {}", result.script);
}

#[test]
fn generate_fix_deduplicates_identical_discrepancies() {
    let ref_col = col_typed("NUMBER");
    let tgt_col = ColumnInfo { data_type: Some("VARCHAR2".to_string()), ..Default::default() };
    let mut servers = make_servers("REF", "T", "C", ref_col);
    let mut tgt_cols: TableColumns = HashMap::new();
    tgt_cols.insert("C".to_string(), tgt_col);
    let mut tgt_tables = HashMap::new();
    tgt_tables.insert("T".to_string(), tgt_cols);
    servers.insert("TGT".to_string(), tgt_tables);

    let discs = vec![
        disc("DIFFERENT", "DATA_TYPE", "T", "C", "TGT", "DATA_TYPE: NUMBER != VARCHAR2"),
        disc("DIFFERENT", "DATA_TYPE", "T", "C", "TGT", "DATA_TYPE: NUMBER != VARCHAR2"),
    ];
    let result = generate_fix_script(&discs, &ids(&[0, 1]), &servers, &HashMap::new(), "REF").unwrap();
    assert_eq!(result.generated_count, 1, "dedup should collapse identical blocks; script: {}", result.script);
}
