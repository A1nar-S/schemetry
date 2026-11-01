use serde_json::json;

use super::*;
use crate::models::DbType;
use crate::services::mcp_sql_guard;

#[test]
fn object_target_dedupes_trims_and_bounds_names() {
    let target = object_target("TABLE", [" ORDERS ", "orders", "ITEMS", ""]);
    assert_eq!(target["names"], json!(["ORDERS", "ITEMS"]));
    assert!(target.get("names_total").is_none());

    let many: Vec<String> = (0..45).map(|i| format!("T{i}")).collect();
    let target = object_target("TABLE", &many);
    assert_eq!(target["names"].as_array().unwrap().len(), MAX_TARGET_NAMES);
    assert_eq!(target["names_total"], 45);
}

#[test]
fn set_columns_is_bounded_but_keeps_the_total() {
    let cols: Vec<String> = (0..100).map(|i| format!("C{i}")).collect();
    let mut target = object_target("TABLE", ["T"]);
    set_columns(&mut target, &cols);
    assert_eq!(target["column_count"], 100);
    assert_eq!(
        target["columns"].as_array().unwrap().len(),
        MAX_TARGET_COLUMNS
    );
}

#[test]
fn very_long_names_are_clipped() {
    let target = object_target("TABLE", ["x".repeat(500)]);
    let name = target["names"][0].as_str().unwrap();
    assert!(name.chars().count() <= 129);
}

fn wrap(target: serde_json::Map<String, serde_json::Value>) -> String {
    json!({ "sql": "x", "target": target }).to_string()
}

#[test]
fn summarize_formats_type_and_names() {
    assert_eq!(
        summarize(&wrap(object_target("TABLE", ["ORDERS", "ITEMS"]))),
        "TABLE ORDERS, ITEMS"
    );
    let many: Vec<String> = (0..25).map(|i| format!("T{i}")).collect();
    let text = summarize(&wrap(object_target("TABLE", &many)));
    assert!(text.ends_with("(+5 more)"), "{text}");
    assert_eq!(
        summarize(&json!({"target": {"names": ["A"]}}).to_string()),
        "A"
    );
}

#[test]
fn summarize_is_empty_for_entries_without_a_target() {
    assert_eq!(summarize("{\"a\":1}"), "");
    assert_eq!(summarize("not json"), "");
    assert_eq!(summarize(""), "");
    assert_eq!(summarize(&json!({"target": {"names": []}}).to_string()), "");
    assert_eq!(summarize(&json!({"target": "oops"}).to_string()), "");
}

// ── table names extracted from SQL ──────────────────────────────────────────────

fn tables(sql: &str) -> Vec<String> {
    mcp_sql_guard::referenced_tables(sql, DbType::Oracle).unwrap()
}

#[test]
fn referenced_tables_lists_joined_tables_once_without_schema_prefixes() {
    assert_eq!(
        tables("SELECT * FROM orders o JOIN app.items i ON i.oid = o.id JOIN ORDERS x ON 1=1"),
        ["ORDERS", "ITEMS"]
    );
    assert_eq!(tables("select 1 from a, b, c"), ["A", "B", "C"]);
}

#[test]
fn referenced_tables_ignores_ctes_and_reads_subqueries() {
    assert_eq!(
        tables("WITH recent AS (SELECT * FROM orders) SELECT * FROM recent r JOIN items i ON 1=1"),
        ["ORDERS", "ITEMS"]
    );
    assert_eq!(
        tables("SELECT * FROM (SELECT id FROM orders) o WHERE o.id IN (SELECT oid FROM items)"),
        ["ORDERS", "ITEMS"]
    );
}

#[test]
fn referenced_tables_handles_quoted_names_and_non_table_selects() {
    assert_eq!(tables("SELECT * FROM \"Mixed Case\""), ["Mixed Case"]);
    assert!(tables("SELECT 1").is_empty());
}

#[test]
fn referenced_tables_rejects_non_select_sql() {
    assert!(mcp_sql_guard::referenced_tables("DELETE FROM orders", DbType::Oracle).is_err());
    assert!(mcp_sql_guard::referenced_tables("SELECT 1; SELECT 2", DbType::Postgres).is_err());
}

#[test]
fn the_object_type_and_the_scanned_inputs_are_bounded() {
    let target = object_target(&"X".repeat(10_000), ["T"]);
    assert_eq!(
        target["object_type"].as_str().unwrap().len(),
        MAX_TYPE_CHARS
    );

    let huge: Vec<String> = (0..100_000).map(|i| format!("T{i}")).collect();
    let target = object_target("TABLE", &huge);
    assert_eq!(target["names"].as_array().unwrap().len(), MAX_TARGET_NAMES);
    assert_eq!(target["names_total"], MAX_SCANNED_NAMES);
}
