//! The audit entry of each tool carries a `target` summary inside its `arguments` JSON.

use serde_json::{Value, json};

use super::*;
use crate::models::{DbType, McpDataAccess, McpPolicy};
use crate::repositories::db_repository::ReadOnlyResult;
use crate::services::mcp_audit_target::summarize;
use crate::services::mcp_query_builder::{Filter, FilterOp};
use crate::test_support::{Harness, allow, connection, enabled_policy};

const CLIENT: &str = "test-agent 1.0";

fn harness() -> Harness {
    let h = Harness::new(vec![connection(1, "Prod", DbType::Oracle)]);
    h.set_policy(McpPolicy {
        data_access: McpDataAccess::Allowlist,
        table_rules: vec![allow("ORDERS", ""), allow("ITEMS", "")],
        allow_raw_sql: true,
        max_rows: 2,
        ..enabled_policy(1)
    });
    for table in ["ORDERS", "ITEMS"] {
        h.db.columns.lock().unwrap().insert(
            table.into(),
            vec!["ID".into(), "STATUS".into(), "TOTAL".into()],
        );
    }
    *h.db.readonly_result.lock().unwrap() = Some(ReadOnlyResult {
        columns: vec!["ID".into()],
        rows: vec![vec![Some("1".into())], vec![Some("2".into())]],
        truncated: true,
    });
    h
}

fn last_arguments(h: &Harness) -> Value {
    serde_json::from_str(&h.entries().last().unwrap().arguments).unwrap()
}

fn query(table: &str) -> QueryTableArgs {
    QueryTableArgs {
        connection_id: 1,
        table: table.to_string(),
        columns: Some(vec!["id".into(), "status".into()]),
        filters: Some(vec![Filter {
            column: "status".into(),
            op: FilterOp::Eq,
            value: Some(json!("OPEN")),
        }]),
        order_by: None,
        limit: Some(5),
    }
}

#[test]
fn query_table_audits_table_columns_filters_limit_and_truncation() {
    let h = harness();
    h.gateway.query_table(CLIENT, query("orders")).unwrap();

    let args = last_arguments(&h);
    let target = &args["target"];
    assert_eq!(target["object_type"], "TABLE");
    assert_eq!(target["names"], json!(["ORDERS"]));
    assert_eq!(target["columns"], json!(["ID", "STATUS"]));
    assert_eq!(target["column_count"], 2);
    assert_eq!(target["filters"], 1);
    // Requested 5, capped by the policy's max_rows.
    assert_eq!(target["limit"], 2);
    assert_eq!(target["truncated"], true);
    assert_eq!(h.entries()[0].row_count, Some(2));
    assert_eq!(summarize(&h.entries()[0].arguments), "TABLE ORDERS");
    // The existing details stay in place.
    assert!(args["sql"].is_string());
}

#[test]
fn a_denied_query_table_still_names_its_table() {
    let h = harness();
    let result = h.gateway.query_table(CLIENT, query("secrets"));
    assert!(matches!(result, Err(ToolError::Denied(_))));

    let entry = h.entries().pop().unwrap();
    assert_eq!(entry.outcome, McpOutcome::Denied);
    assert_eq!(summarize(&entry.arguments), "TABLE SECRETS");
}

#[test]
fn run_query_audits_the_tables_the_sql_reads() {
    let h = harness();
    h.gateway
        .run_query(
            CLIENT,
            RunQueryArgs {
                connection_id: 1,
                sql: "SELECT o.id FROM orders o JOIN app.items i ON i.oid = o.id JOIN orders x ON 1=1"
                    .into(),
                max_rows: None,
            },
        )
        .unwrap();

    let target = last_arguments(&h)["target"].clone();
    assert_eq!(target["names"], json!(["ORDERS", "ITEMS"]));
    assert_eq!(target["truncated"], true);
    assert_eq!(target["limit"], 2);
    assert_eq!(summarize(&h.entries()[0].arguments), "TABLE ORDERS, ITEMS");
}

#[test]
fn run_query_denied_for_a_table_outside_the_allowlist_still_records_it() {
    let h = harness();
    let result = h.gateway.run_query(
        CLIENT,
        RunQueryArgs {
            connection_id: 1,
            sql: "SELECT * FROM payroll".into(),
            max_rows: None,
        },
    );
    assert!(matches!(result, Err(ToolError::Denied(_))));
    assert_eq!(summarize(&h.entries()[0].arguments), "TABLE PAYROLL");
}

#[test]
fn get_object_ddl_audits_object_type_and_name_even_when_refused() {
    let h = harness();
    h.db.ddls.lock().unwrap().insert(
        "VIEW:V_ORDERS".into(),
        "CREATE VIEW V_ORDERS AS SELECT 1 FROM dual".into(),
    );

    h.gateway
        .get_object_ddl(
            CLIENT,
            GetObjectDdlArgs {
                connection_id: 1,
                object_name: "V_ORDERS".into(),
                object_type: "view".into(),
            },
        )
        .unwrap();
    assert_eq!(summarize(&h.entries()[0].arguments), "VIEW V_ORDERS");

    let refused = h.gateway.get_object_ddl(
        CLIENT,
        GetObjectDdlArgs {
            connection_id: 1,
            object_name: "BAD; DROP".into(),
            object_type: "TABLE".into(),
        },
    );
    assert!(matches!(refused, Err(ToolError::Denied(_))));
    assert_eq!(summarize(&h.entries()[1].arguments), "TABLE BAD; DROP");
}

#[test]
fn an_oversized_argument_blob_keeps_the_target() {
    let mut arguments = json!({ "sql": "x".repeat(MAX_AUDIT_ARGUMENT_BYTES + 10) });
    arguments["target"] = json!({ "object_type": "TABLE", "names": ["ORDERS"] });
    let bounded = bounded_arguments(arguments);
    assert!(bounded.contains("\"truncated\":true"));
    assert_eq!(summarize(&bounded), "TABLE ORDERS");
}
