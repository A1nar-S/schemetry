use super::*;
use std::collections::HashMap;
use crate::models::{ColumnInfo, TableColumns};

fn opt(s: &str) -> Option<String> {
    Some(s.to_string())
}

fn col_info(data_type: &str) -> ColumnInfo {
    ColumnInfo {
        data_type: Some(data_type.to_string()),
        ..Default::default()
    }
}

fn make_servers(data: &[(&str, &[(&str, &[(&str, ColumnInfo)])])]) -> ServersData {
    let mut servers: ServersData = HashMap::new();
    for (server, tables) in data {
        let mut server_tables: HashMap<String, TableColumns> = HashMap::new();
        for (table, cols) in *tables {
            let mut col_map: TableColumns = HashMap::new();
            for (name, info) in *cols {
                col_map.insert(name.to_string(), info.clone());
            }
            server_tables.insert(table.to_string(), col_map);
        }
        servers.insert(server.to_string(), server_tables);
    }
    servers
}

// ── equal_ci ─────────────────────────────────────────────────────

#[test]
fn equal_ci_both_none() {
    assert!(equal_ci(&None, &None));
}

#[test]
fn equal_ci_left_some_right_none() {
    assert!(!equal_ci(&opt("abc"), &None));
}

#[test]
fn equal_ci_left_none_right_some() {
    assert!(!equal_ci(&None, &opt("abc")));
}

#[test]
fn equal_ci_same_value() {
    assert!(equal_ci(&opt("VARCHAR2"), &opt("VARCHAR2")));
}

#[test]
fn equal_ci_different_case() {
    assert!(equal_ci(&opt("varchar2"), &opt("VARCHAR2")));
}

#[test]
fn equal_ci_different_value() {
    assert!(!equal_ci(&opt("NUMBER"), &opt("VARCHAR2")));
}

// ── skip_index_diff ──────────────────────────────────────────────

#[test]
fn skip_index_diff_both_none() {
    assert!(!skip_index_diff(&None, &None));
}

#[test]
fn skip_index_diff_one_none() {
    assert!(!skip_index_diff(&opt("IDX1"), &None));
    assert!(!skip_index_diff(&None, &opt("IDX1")));
}

#[test]
fn skip_index_diff_both_none_strings() {
    assert!(!skip_index_diff(&opt("none"), &opt("NONE")));
}

#[test]
fn skip_index_diff_one_is_none_string() {
    assert!(!skip_index_diff(&opt("IDX1"), &opt("none")));
    assert!(!skip_index_diff(&opt("none"), &opt("IDX1")));
}

#[test]
fn skip_index_diff_both_real_indexes() {
    assert!(skip_index_diff(&opt("IDX_REF"), &opt("IDX_TGT")));
}

// ── compare_tables_across_servers ────────────────────────────────

#[test]
fn compare_unknown_reference_returns_err() {
    let servers = make_servers(&[("SERVER_A", &[])]);
    assert!(compare_tables_across_servers(&servers, "UNKNOWN", false, false).is_err());
}

#[test]
fn compare_identical_schemas_no_discrepancies() {
    let info = col_info("NUMBER");
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", info.clone())])]),
        ("TGT", &[("T", &[("C1", info)])]),
    ]);
    let result = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert!(result.is_empty(), "expected no discrepancies, got: {result:?}");
}

#[test]
fn compare_table_missing_in_target() {
    let servers = make_servers(&[
        ("REF", &[("ORDERS", &[("ID", col_info("NUMBER"))])]),
        ("TGT", &[]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].difference, "MISSING");
    assert_eq!(discs[0].element, "TABLE");
    assert_eq!(discs[0].table_name, "ORDERS");
    assert_eq!(discs[0].server_name, "TGT");
}

#[test]
fn compare_table_missing_in_reference() {
    let servers = make_servers(&[
        ("REF", &[]),
        ("TGT", &[("EXTRA", &[("ID", col_info("NUMBER"))])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].difference, "MISSING");
    assert_eq!(discs[0].element, "TABLE");
    assert_eq!(discs[0].server_name, "TGT");
}

#[test]
fn compare_column_missing_in_target() {
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", col_info("NUMBER")), ("C2", col_info("VARCHAR2"))])]),
        ("TGT", &[("T", &[("C1", col_info("NUMBER"))])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].element, "COLUMN");
    assert_eq!(discs[0].column_name, "C2");
    assert_eq!(discs[0].server_name, "TGT");
}

#[test]
fn compare_column_missing_in_reference() {
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", col_info("NUMBER"))])]),
        ("TGT", &[("T", &[("C1", col_info("NUMBER")), ("C2", col_info("DATE"))])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].element, "COLUMN");
    assert_eq!(discs[0].column_name, "C2");
    assert_eq!(discs[0].server_name, "TGT");
}

#[test]
fn compare_data_type_mismatch() {
    let ref_info = ColumnInfo { data_type: Some("NUMBER".to_string()), ..Default::default() };
    let tgt_info = ColumnInfo { data_type: Some("VARCHAR2".to_string()), ..Default::default() };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].difference, "DIFFERENT");
    assert_eq!(discs[0].element, "DATA_TYPE");
}

#[test]
fn compare_comment_mismatch_ignored_when_flag_off() {
    let ref_info = ColumnInfo { comments: Some("Doc".to_string()), ..col_info("NUMBER") };
    let tgt_info = ColumnInfo { comments: Some("Other".to_string()), ..col_info("NUMBER") };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    assert!(compare_tables_across_servers(&servers, "REF", false, false).unwrap().is_empty());
}

#[test]
fn compare_comment_mismatch_detected_when_flag_on() {
    let ref_info = ColumnInfo { comments: Some("Doc".to_string()), ..col_info("NUMBER") };
    let tgt_info = ColumnInfo { comments: Some("Other".to_string()), ..col_info("NUMBER") };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", true, false).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].element, "COMMENTS");
}

#[test]
fn compare_index_mismatch_ignored_when_flag_off() {
    // ref has real index; target has "none" → should be detected, but flag is off so skipped
    let ref_info = ColumnInfo { index_name: Some("IDX_A".to_string()), ..col_info("NUMBER") };
    let tgt_info = ColumnInfo { index_name: Some("none".to_string()), ..col_info("NUMBER") };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    assert!(compare_tables_across_servers(&servers, "REF", false, false).unwrap().is_empty());
}

#[test]
fn compare_index_mismatch_detected_when_flag_on() {
    // ref has real index; target has "none" → detected when flag is on
    let ref_info = ColumnInfo { index_name: Some("IDX_A".to_string()), ..col_info("NUMBER") };
    let tgt_info = ColumnInfo { index_name: Some("none".to_string()), ..col_info("NUMBER") };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    let discs = compare_tables_across_servers(&servers, "REF", false, true).unwrap();
    assert_eq!(discs.len(), 1);
    assert_eq!(discs[0].element, "INDEX_NAME");
}

#[test]
fn compare_both_real_index_names_differ_are_skipped() {
    // skip_index_diff returns true when both sides have non-"none" index names,
    // so differing real index names are intentionally not flagged
    let ref_info = ColumnInfo { index_name: Some("IDX_A".to_string()), ..col_info("NUMBER") };
    let tgt_info = ColumnInfo { index_name: Some("IDX_B".to_string()), ..col_info("NUMBER") };
    let servers = make_servers(&[
        ("REF", &[("T", &[("C1", ref_info)])]),
        ("TGT", &[("T", &[("C1", tgt_info)])]),
    ]);
    assert!(compare_tables_across_servers(&servers, "REF", false, true).unwrap().is_empty());
}
