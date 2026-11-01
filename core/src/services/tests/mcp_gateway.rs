use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use super::*;
use crate::models::{
    ColumnInfo, DbType, McpDataAccess, McpObjectDenyRule, McpPolicy, SchemaObject,
};
use crate::repositories::db_repository::ReadOnlyResult;
use crate::services::mcp_query_builder::{Filter, FilterOp, OrderBy};
use crate::test_support::{Harness, allow, connection, deny, enabled_policy};

const CLIENT: &str = "test-agent 1.0";
const PASSWORD: &str = "s3cret-password";

fn data_policy(id: i64, rules: Vec<crate::models::McpTableRule>) -> McpPolicy {
    McpPolicy {
        data_access: McpDataAccess::Allowlist,
        table_rules: rules,
        ..enabled_policy(id)
    }
}

fn oracle() -> Harness {
    Harness::new(vec![connection(1, "Prod", DbType::Oracle)])
}

fn postgres() -> Harness {
    let mut conn = connection(1, "Prod", DbType::Postgres);
    conn.pg_schema = "sales".into();
    Harness::new(vec![conn])
}

fn query(table: &str) -> QueryTableArgs {
    QueryTableArgs {
        connection_id: 1,
        table: table.to_string(),
        columns: None,
        filters: None,
        order_by: None,
        limit: None,
    }
}

fn raw(sql: &str) -> RunQueryArgs {
    RunQueryArgs {
        connection_id: 1,
        sql: sql.to_string(),
        max_rows: None,
    }
}

fn rows(n: usize, truncated: bool) -> ReadOnlyResult {
    ReadOnlyResult {
        columns: vec!["ID".into(), "STATUS".into()],
        rows: (0..n)
            .map(|i| vec![Some(i.to_string()), Some("OPEN".into())])
            .collect(),
        truncated,
    }
}

fn has_tables(h: &Harness, names: &[&str]) {
    for name in names {
        h.db.columns
            .lock()
            .unwrap()
            .insert(name.to_string(), vec!["ID".into()]);
    }
}

fn prepare_orders(h: &Harness) {
    h.db.columns.lock().unwrap().insert(
        "ORDERS".into(),
        vec!["ID".into(), "STATUS".into(), "PASSWORD_HASH".into()],
    );
    *h.db.readonly_result.lock().unwrap() = Some(rows(2, false));
}

fn denied_message(result: ToolResult) -> String {
    match result {
        Err(ToolError::Denied(message)) => message,
        _ => panic!("expected a denial"),
    }
}

fn failed_message(result: ToolResult) -> String {
    match result {
        Err(ToolError::Failed(message)) => message,
        _ => panic!("expected a failure"),
    }
}

/// Asserts the last audit entry's outcome and that exactly `count` entries exist.
fn assert_audit(h: &Harness, count: usize, outcome: McpOutcome) {
    let entries = h.entries();
    assert_eq!(entries.len(), count, "unexpected number of audit entries");
    assert_eq!(entries.last().unwrap().outcome, outcome);
}

// ── list_connections ────────────────────────────────────────────────────────────

#[test]
fn list_connections_shows_only_enabled_connections_and_never_credentials() {
    let h = Harness::new(vec![
        connection(1, "Prod", DbType::Oracle),
        connection(2, "Hidden", DbType::Oracle),
    ]);
    h.set_policy(McpPolicy {
        mcp_username: "svc_mcp".into(),
        ..data_policy(1, vec![allow("ORDERS", "")])
    });

    let listed = h.gateway.list_connections(CLIENT).unwrap();
    let text = listed.to_string();
    let connections = listed["connections"].as_array().unwrap();
    assert_eq!(connections.len(), 1);
    assert_eq!(connections[0]["id"], 1);
    assert_eq!(connections[0]["name"], "Prod");
    assert_eq!(connections[0]["db_type"], "oracle");
    assert_eq!(connections[0]["schema"], "APP");
    assert_eq!(connections[0]["policy"]["data_access"], "allowlist");
    for secret in [PASSWORD, "db.example", "svc_mcp", "Hidden"] {
        assert!(!text.contains(secret), "a sensitive value leaked");
    }
    assert_audit(&h, 1, McpOutcome::Allowed);
    assert_eq!(h.entries()[0].tool, "list_connections");
}

#[test]
fn list_connections_explains_why_each_tool_is_unavailable() {
    let h = Harness::new(vec![
        connection(53, "Reader", DbType::Oracle),
        connection(54, "Saver", DbType::Oracle),
    ]);
    h.set_policy(enabled_policy(53));
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        ..enabled_policy(54)
    });

    let listed = h.gateway.list_connections(CLIENT).unwrap();
    let policy_of = |id: i64| {
        listed["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap()["policy"]
            .clone()
    };
    let reader = policy_of(53);
    assert_eq!(reader["tools"]["get_object_ddl"]["allowed"], true);
    assert_eq!(reader["tools"]["save_migration_script"]["allowed"], false);
    assert!(
        reader["tools"]["save_migration_script"]["reason"]
            .as_str()
            .unwrap()
            .contains("allow_save_scripts=off")
    );
    let saver = policy_of(54);
    assert_eq!(saver["tools"]["save_migration_script"]["allowed"], true);
    assert_eq!(
        saver["tools"]["query_table"]["reason_code"],
        "data_access_none"
    );
    assert_eq!(saver["data_access"], "none");

    assert_eq!(listed["data_tools_disabled_globally"], false);
    assert!(
        listed["hint"]
            .as_str()
            .unwrap()
            .contains("Connections > MCP policy")
    );
}

#[test]
fn list_connections_reflects_the_global_kill_switch() {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_raw_sql: true,
        ..data_policy(1, vec![allow("*", "")])
    });
    h.settings.data_tools_disabled.store(true, Ordering::SeqCst);

    let listed = h.gateway.list_connections(CLIENT).unwrap();
    assert_eq!(listed["data_tools_disabled_globally"], true);
    let tools = &listed["connections"][0]["policy"]["tools"];
    assert_eq!(tools["query_table"]["reason_code"], "kill_switch");
    assert_eq!(tools["run_query"]["allowed"], false);
    assert_eq!(tools["list_schema_objects"]["allowed"], true);

    h.settings
        .data_tools_disabled
        .store(false, Ordering::SeqCst);
    let listed = h.gateway.list_connections(CLIENT).unwrap();
    assert_eq!(
        listed["connections"][0]["policy"]["tools"]["run_query"]["allowed"],
        true
    );
}

#[test]
fn list_connections_view_agrees_with_what_the_gateway_enforces() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);
    let listed = h.gateway.list_connections(CLIENT).unwrap();
    let tools = &listed["connections"][0]["policy"]["tools"];
    assert_eq!(tools["query_table"]["allowed"], true);
    assert!(h.gateway.query_table(CLIENT, query("ORDERS")).is_ok());
    assert_eq!(tools["run_query"]["allowed"], false);
    assert!(
        h.gateway
            .run_query(CLIENT, raw("SELECT 1 FROM DUAL"))
            .is_err()
    );
}

#[test]
fn existing_connections_without_a_policy_are_invisible() {
    let h = oracle();
    let listed = h.gateway.list_connections(CLIENT).unwrap();
    assert!(listed["connections"].as_array().unwrap().is_empty());
}

// ── gating ──────────────────────────────────────────────────────────────────────

#[test]
fn unknown_and_disabled_connections_are_denied_identically_without_touching_the_db() {
    let h = oracle();
    h.set_policy(McpPolicy {
        mcp_enabled: false,
        ..data_policy(1, vec![allow("*", "")])
    });
    let args = |id| ListSchemaObjectsArgs {
        connection_id: id,
        object_type: None,
    };

    let disabled = denied_message(h.gateway.list_schema_objects(CLIENT, args(1)));
    let unknown = denied_message(h.gateway.list_schema_objects(CLIENT, args(99)));
    assert_eq!(disabled.replace('1', "N"), unknown.replace("99", "N"));
    assert_eq!(h.db_calls(), 0);
    assert_audit(&h, 2, McpOutcome::Denied);
}

#[test]
fn default_policy_denies_all_data_access() {
    let h = oracle();
    h.set_policy(enabled_policy(1));
    prepare_orders(&h);

    let message = denied_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    assert!(message.contains("data_access"));
    let message = denied_message(h.gateway.run_query(CLIENT, raw("SELECT 1 FROM DUAL")));
    assert!(message.contains("not permitted"));

    assert_eq!(h.db_calls(), 0);
    assert!(h.db.seen.lock().unwrap().is_empty());
    assert_audit(&h, 2, McpOutcome::Denied);
}

#[test]
fn capability_flags_gate_each_tool() {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_schema_list: false,
        allow_ddl_read: false,
        ..enabled_policy(1)
    });
    assert!(matches!(
        h.gateway.list_schema_objects(
            CLIENT,
            ListSchemaObjectsArgs {
                connection_id: 1,
                object_type: None
            }
        ),
        Err(ToolError::Denied(_))
    ));
    assert!(matches!(
        h.gateway.get_object_ddl(
            CLIENT,
            GetObjectDdlArgs {
                connection_id: 1,
                object_name: "ORDERS".into(),
                object_type: "TABLE".into()
            }
        ),
        Err(ToolError::Denied(_))
    ));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn the_global_kill_switch_disables_data_tools_but_not_schema_tools() {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_raw_sql: true,
        ..data_policy(1, vec![allow("*", "")])
    });
    prepare_orders(&h);
    h.settings.data_tools_disabled.store(true, Ordering::SeqCst);

    let q = denied_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    let r = denied_message(h.gateway.run_query(CLIENT, raw("SELECT * FROM orders")));
    assert!(q.contains("switched off globally"));
    assert!(r.contains("switched off globally"));
    assert_eq!(h.db_calls(), 0);

    *h.db.objects.lock().unwrap() = vec![SchemaObject {
        name: "ORDERS".into(),
        object_type: "TABLE".into(),
    }];
    assert!(
        h.gateway
            .list_schema_objects(
                CLIENT,
                ListSchemaObjectsArgs {
                    connection_id: 1,
                    object_type: None
                }
            )
            .is_ok()
    );

    h.settings
        .data_tools_disabled
        .store(false, Ordering::SeqCst);
    assert!(h.gateway.query_table(CLIENT, query("ORDERS")).is_ok());
}

#[test]
fn a_broken_keychain_password_fails_without_reaching_the_db() {
    let mut conn = connection(1, "Prod", DbType::Oracle);
    conn.password_broken = true;
    let h = Harness::new(vec![conn]);
    h.set_policy(enabled_policy(1));
    let message = failed_message(h.gateway.list_schema_objects(
        CLIENT,
        ListSchemaObjectsArgs {
            connection_id: 1,
            object_type: None,
        },
    ));
    assert!(message.contains("keychain"));
    assert_eq!(h.db_calls(), 0);
    assert_audit(&h, 1, McpOutcome::Error);
}

// ── schema tools ────────────────────────────────────────────────────────────────

#[test]
fn schema_listing_hides_denied_objects_and_filters_by_type() {
    let h = oracle();
    h.set_policy(McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "AUDIT_*".into(),
        }],
        ..enabled_policy(1)
    });
    *h.db.objects.lock().unwrap() = ["ORDERS", "AUDIT_LOG", "ITEMS"]
        .iter()
        .map(|n| SchemaObject {
            name: n.to_string(),
            object_type: "TABLE".into(),
        })
        .chain([SchemaObject {
            name: "ORDERS_VW".into(),
            object_type: "VIEW".into(),
        }])
        .collect();

    let all = h
        .gateway
        .list_schema_objects(
            CLIENT,
            ListSchemaObjectsArgs {
                connection_id: 1,
                object_type: None,
            },
        )
        .unwrap();
    let names: Vec<&str> = all["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ORDERS", "ITEMS", "ORDERS_VW"]);
    assert_eq!(all["count"], 3);

    let views = h
        .gateway
        .list_schema_objects(
            CLIENT,
            ListSchemaObjectsArgs {
                connection_id: 1,
                object_type: Some("view".into()),
            },
        )
        .unwrap();
    assert_eq!(views["count"], 1);
}

#[test]
fn ddl_of_hidden_objects_is_refused_and_others_are_returned() {
    let h = oracle();
    h.set_policy(McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "TABLE".into(),
            name: "AUDIT_*".into(),
        }],
        ..enabled_policy(1)
    });
    h.db.ddls.lock().unwrap().insert(
        "TABLE:ORDERS".into(),
        "CREATE TABLE ORDERS (ID NUMBER);".into(),
    );
    let args = |name: &str, ty: &str| GetObjectDdlArgs {
        connection_id: 1,
        object_name: name.into(),
        object_type: ty.into(),
    };

    let ok = h
        .gateway
        .get_object_ddl(CLIENT, args("orders", "table"))
        .unwrap();
    assert_eq!(ok["ddl"], "CREATE TABLE ORDERS (ID NUMBER);");

    let hidden = denied_message(h.gateway.get_object_ddl(CLIENT, args("AUDIT_LOG", "TABLE")));
    assert!(hidden.contains("hidden"));
    let calls = h.db_calls();
    // A bad name never reaches the database either.
    denied_message(
        h.gateway
            .get_object_ddl(CLIENT, args("ORDERS; DROP", "TABLE")),
    );
    denied_message(
        h.gateway
            .get_object_ddl(CLIENT, args("ORDERS", "TABLE; --")),
    );
    assert_eq!(h.db_calls(), calls);
    assert_audit(&h, 4, McpOutcome::Denied);
}

// ── invalid objects ─────────────────────────────────────────────────────────────

fn invalid(name: &str, object_type: &str) -> crate::models::InvalidObject {
    crate::models::InvalidObject {
        name: name.into(),
        object_type: object_type.into(),
        last_ddl_time: Some("2026-01-02 03:04:05".into()),
    }
}

fn errors_args(name: &str, ty: &str) -> GetObjectErrorsArgs {
    GetObjectErrorsArgs {
        connection_id: 1,
        object_name: name.into(),
        object_type: ty.into(),
    }
}

fn invalid_args(object_type: Option<&str>) -> ListInvalidObjectsArgs {
    ListInvalidObjectsArgs {
        connection_id: 1,
        object_type: object_type.map(Into::into),
    }
}

#[test]
fn list_invalid_objects_filters_by_type_hides_denied_and_audits_counts() {
    let h = oracle();
    h.set_policy(McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "SECRET_*".into(),
        }],
        ..enabled_policy(1)
    });
    *h.db.invalid_objects.lock().unwrap() = vec![
        invalid("PKG_ORDERS", "PACKAGE BODY"),
        invalid("SECRET_PKG", "PACKAGE BODY"),
        invalid("V_ORDERS", "VIEW"),
    ];

    let all = h
        .gateway
        .list_invalid_objects(CLIENT, invalid_args(None))
        .unwrap();
    let names: Vec<&str> = all["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["PKG_ORDERS", "V_ORDERS"]);
    assert_eq!(all["count"], 2);
    assert_eq!(all["applicable"], true);

    let views = h
        .gateway
        .list_invalid_objects(CLIENT, invalid_args(Some("view")))
        .unwrap();
    assert_eq!(views["count"], 1);

    let entries = h.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].tool, "list_invalid_objects");
    assert_eq!(entries[0].row_count, Some(2));
    let arguments: Value = serde_json::from_str(&entries[0].arguments).unwrap();
    assert_eq!(arguments["invalid_count"], 2);
    assert_eq!(arguments["hidden_by_policy"], 1);
    assert!(!entries[0].arguments.contains("SECRET_PKG"));
}

#[test]
fn list_invalid_objects_rejects_malformed_type_and_respects_the_capability_flag() {
    let h = oracle();
    h.set_policy(enabled_policy(1));
    let message = denied_message(
        h.gateway
            .list_invalid_objects(CLIENT, invalid_args(Some("VIEW'; --"))),
    );
    assert!(message.contains("object_type"));
    assert_eq!(h.db_calls(), 0);

    h.set_policy(McpPolicy {
        allow_invalid_objects: false,
        ..enabled_policy(1)
    });
    let message = denied_message(h.gateway.list_invalid_objects(CLIENT, invalid_args(None)));
    assert!(message.contains("allow_invalid_objects"));
    let message = denied_message(
        h.gateway
            .get_object_errors(CLIENT, errors_args("P", "PACKAGE")),
    );
    assert!(message.contains("allow_invalid_objects"));
    assert_eq!(h.db_calls(), 0);
    assert_audit(&h, 3, McpOutcome::Denied);
}

#[test]
fn invalid_object_tools_are_not_applicable_on_postgres() {
    let h = postgres();
    h.set_policy(enabled_policy(1));
    let list = h
        .gateway
        .list_invalid_objects(CLIENT, invalid_args(None))
        .unwrap();
    assert_eq!(list["applicable"], false);
    assert_eq!(list["count"], 0);
    assert!(list["message"].is_string());
    let errors = h
        .gateway
        .get_object_errors(CLIENT, errors_args("FN_X", "FUNCTION"))
        .unwrap();
    assert_eq!(errors["applicable"], false);
    assert_eq!(errors["errors"], json!([]));
    assert_audit(&h, 2, McpOutcome::Allowed);
}

#[test]
fn get_object_errors_returns_diagnostics_with_counts() {
    use crate::models::{ObjectError, ObjectErrorList};
    let h = oracle();
    h.set_policy(enabled_policy(1));
    h.db.object_errors.lock().unwrap().insert(
        "PACKAGE BODY:PKG_ORDERS".into(),
        ObjectErrorList {
            applicable: true,
            message: None,
            status: Some("INVALID".into()),
            errors: vec![
                ObjectError {
                    line: 12,
                    position: 5,
                    attribute: "ERROR".into(),
                    message: "PLS-00201: identifier 'X' must be declared".into(),
                },
                ObjectError {
                    line: 20,
                    position: 1,
                    attribute: "WARNING".into(),
                    message: "PLW-05018: unit omitted its AUTHID clause".into(),
                },
            ],
            truncated: false,
        },
    );

    let out = h
        .gateway
        .get_object_errors(CLIENT, errors_args("pkg_orders", "package body"))
        .unwrap();
    assert_eq!(out["object_type"], "PACKAGE BODY");
    assert_eq!(out["status"], "INVALID");
    assert_eq!(out["error_count"], 1);
    assert_eq!(out["warning_count"], 1);
    assert_eq!(out["errors"][0]["line"], 12);
    assert_eq!(out["errors"][0]["position"], 5);

    let entries = h.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tool, "get_object_errors");
    assert_eq!(entries[0].row_count, Some(2));
    let arguments: Value = serde_json::from_str(&entries[0].arguments).unwrap();
    assert_eq!(arguments["error_count"], 1);
    assert_eq!(arguments["warning_count"], 1);
}

#[test]
fn get_object_errors_refuses_hidden_and_malformed_requests_without_touching_the_database() {
    let h = oracle();
    h.set_policy(McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "PACKAGE".into(),
            name: "SECRET_*".into(),
        }],
        ..enabled_policy(1)
    });
    let hidden = denied_message(
        h.gateway
            .get_object_errors(CLIENT, errors_args("SECRET_PKG", "PACKAGE")),
    );
    assert!(hidden.contains("hidden"));
    denied_message(
        h.gateway
            .get_object_errors(CLIENT, errors_args("A; DROP", "VIEW")),
    );
    denied_message(
        h.gateway
            .get_object_errors(CLIENT, errors_args("V", "VIEW; --")),
    );
    assert_eq!(h.db_calls(), 0);
    assert_audit(&h, 3, McpOutcome::Denied);
}

// ΓöÇΓöÇ package spec / body ΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇΓöÇ

const PKG_SPEC: &str =
    "CREATE OR REPLACE PACKAGE ORDER_API AS\n  PROCEDURE run;\nEND ORDER_API;\n/";
const PKG_BODY: &str = "CREATE OR REPLACE PACKAGE BODY ORDER_API AS\n  PROCEDURE run IS BEGIN NULL; END;\nEND ORDER_API;\n/";

fn package_harness() -> Harness {
    let h = oracle();
    h.set_policy(enabled_policy(1));
    let mut ddls = h.db.ddls.lock().unwrap();
    ddls.insert("PACKAGE:ORDER_API".into(), PKG_SPEC.into());
    ddls.insert("PACKAGE BODY:ORDER_API".into(), PKG_BODY.into());
    drop(ddls);
    h
}

fn ddl_args(ty: &str) -> GetObjectDdlArgs {
    GetObjectDdlArgs {
        connection_id: 1,
        object_name: "order_api".into(),
        object_type: ty.into(),
    }
}

#[test]
fn package_types_state_which_parts_they_include() {
    let h = package_harness();

    let spec = h
        .gateway
        .get_object_ddl(CLIENT, ddl_args("package"))
        .unwrap();
    assert_eq!(spec["includes"], json!(["spec"]));
    assert_eq!(spec["ddl"], PKG_SPEC);

    let body = h
        .gateway
        .get_object_ddl(CLIENT, ddl_args("PACKAGE BODY"))
        .unwrap();
    assert_eq!(body["includes"], json!(["body"]));
    assert_eq!(body["ddl"], PKG_BODY);

    h.db.ddls.lock().unwrap().insert(
        "TABLE:ORDERS".into(),
        "CREATE TABLE ORDERS (ID NUMBER);".into(),
    );
    let table = h
        .gateway
        .get_object_ddl(
            CLIENT,
            GetObjectDdlArgs {
                object_name: "ORDERS".into(),
                ..ddl_args("TABLE")
            },
        )
        .unwrap();
    assert!(table.get("includes").is_none());
}

#[test]
fn combined_package_type_returns_spec_then_body_separated_by_a_slash_line() {
    let h = package_harness();
    let out = h
        .gateway
        .get_object_ddl(CLIENT, ddl_args("package spec and body"))
        .unwrap();

    assert_eq!(out["object_type"], "PACKAGE SPEC AND BODY");
    assert_eq!(out["includes"], json!(["spec", "body"]));
    let ddl = out["ddl"].as_str().unwrap();
    assert_eq!(
        ddl,
        format!(
            "{}\n/\n\n{}\n/\n",
            PKG_SPEC.trim_end_matches('/').trim_end(),
            PKG_BODY.trim_end_matches('/').trim_end()
        )
    );
    assert!(ddl.find("PACKAGE ORDER_API").unwrap() < ddl.find("PACKAGE BODY ORDER_API").unwrap());
    assert!(crate::services::ddl_save::has_package_spec_and_body(ddl));
}

#[test]
fn combined_package_type_explains_a_missing_body() {
    let h = package_harness();
    h.db.ddls.lock().unwrap().remove("PACKAGE BODY:ORDER_API");
    let message = failed_message(
        h.gateway
            .get_object_ddl(CLIENT, ddl_args("PACKAGE SPEC AND BODY")),
    );
    assert!(message.contains("no body"), "{message}");
    assert_audit(&h, 1, McpOutcome::Error);
}

#[test]
fn combined_package_type_is_rejected_for_postgres() {
    let h = postgres();
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        ..enabled_policy(1)
    });
    let root = tempfile::tempdir().unwrap();
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());

    let read = denied_message(
        h.gateway
            .get_object_ddl(CLIENT, ddl_args("PACKAGE SPEC AND BODY")),
    );
    assert!(
        read.contains("Oracle") && read.contains("PostgreSQL"),
        "{read}"
    );
    let mut save = save_args("ORDER_API", "pkg");
    save.object_type = "PACKAGE SPEC AND BODY".into();
    save.ddl = Some(format!("{PKG_SPEC}\n{PKG_BODY}"));
    let saved = denied_message(h.gateway.save_migration_script(CLIENT, save));
    assert!(saved.contains("Oracle"), "{saved}");
    assert_eq!(h.db_calls(), 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn hiding_either_package_half_hides_the_combined_type() {
    for hidden_type in ["PACKAGE", "PACKAGE BODY", "PACKAGE SPEC AND BODY"] {
        let h = package_harness();
        h.set_policy(McpPolicy {
            object_deny: vec![McpObjectDenyRule {
                object_type: hidden_type.into(),
                name: "ORDER_API".into(),
            }],
            ..enabled_policy(1)
        });
        let message = denied_message(
            h.gateway
                .get_object_ddl(CLIENT, ddl_args("PACKAGE SPEC AND BODY")),
        );
        assert!(message.contains("hidden"), "{hidden_type}: {message}");
        assert_eq!(h.db_calls(), 0);
    }
}

#[test]
fn saving_the_combined_package_type_writes_one_pck_file() {
    let (h, root) = save_harness();
    let mut args = save_args("ORDER_API", "pkg");
    args.object_type = "package spec and body".into();
    args.ddl = Some(crate::services::ddl_save::combine_package_spec_and_body(
        PKG_SPEC, PKG_BODY,
    ));
    args.dry_run = true;
    let out = h
        .gateway
        .save_migration_script(CLIENT, args.clone())
        .unwrap();
    let code = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "code")
        .unwrap();
    assert_eq!(code["content"], args.ddl.clone().unwrap());
    assert!(code["path"].as_str().unwrap().ends_with("ORDER_API.sql"));

    args.dry_run = false;
    h.gateway
        .save_migration_script(CLIENT, args.clone())
        .unwrap();
    let saved = std::fs::read_to_string(
        root.path()
            .join("app")
            .join("code")
            .join("app")
            .join("ORDER_API.sql"),
    )
    .unwrap();
    assert_eq!(saved, args.ddl.clone().unwrap());
}

#[test]
fn saving_the_combined_package_type_requires_spec_and_body() {
    let (h, root) = save_harness();
    for ddl in [PKG_SPEC, PKG_BODY, "CREATE TABLE ORDERS (ID NUMBER);"] {
        let mut args = save_args("ORDER_API", "pkg");
        args.object_type = "PACKAGE SPEC AND BODY".into();
        args.ddl = Some(ddl.into());
        let message = denied_message(h.gateway.save_migration_script(CLIENT, args));
        assert!(message.contains("CREATE PACKAGE BODY"), "{message}");
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

// ── query_table ─────────────────────────────────────────────────────────────────

#[test]
fn query_table_runs_a_read_only_bounded_query_for_allowed_tables() {
    let h = oracle();
    h.set_policy(McpPolicy {
        max_rows: 50,
        query_timeout_secs: 12,
        ..data_policy(1, vec![allow("orders", ""), deny("ORDERS", "PASSWORD*")])
    });
    prepare_orders(&h);

    let out = h.gateway.query_table(CLIENT, query("orders")).unwrap();
    assert_eq!(out["columns"], json!(["ID", "STATUS"]));
    assert_eq!(out["row_count"], 2);
    assert_eq!(out["truncated"], false);
    assert_eq!(out["omitted_columns"], 1);
    assert_eq!(out["rows"][0], json!(["0", "OPEN"]));

    let seen = h.db.seen.lock().unwrap()[0].clone();
    assert_eq!(seen.max_rows, 50);
    assert_eq!(seen.timeout_secs, 12);
    assert!(seen.sql.contains("\"ID\", \"STATUS\""));
    assert!(!seen.sql.contains("PASSWORD"));
    assert!(seen.sql.contains("\"APP\".\"ORDERS\""));
    assert_audit(&h, 1, McpOutcome::Allowed);
    assert_eq!(h.entries()[0].row_count, Some(2));
}

#[test]
fn tables_outside_the_allowlist_are_denied_before_any_catalog_access() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);

    let message = denied_message(h.gateway.query_table(CLIENT, query("CUSTOMERS")));
    assert!(message.contains("CUSTOMERS"));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn denied_tables_win_over_allow_everything() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("*", ""), deny("ORDERS", "")]));
    prepare_orders(&h);
    denied_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn requesting_a_denied_column_is_an_error_not_a_silent_omission() {
    let h = oracle();
    h.set_policy(data_policy(
        1,
        vec![allow("ORDERS", ""), deny("*", "*PASSWORD*")],
    ));
    prepare_orders(&h);

    let mut args = query("ORDERS");
    args.columns = Some(vec!["id".into(), "password_hash".into()]);
    let message = denied_message(h.gateway.query_table(CLIENT, args));
    assert!(message.contains("PASSWORD_HASH"));
    assert!(h.db.seen.lock().unwrap().is_empty());
}

#[test]
fn filtering_or_ordering_on_a_denied_column_is_refused() {
    let h = oracle();
    h.set_policy(data_policy(
        1,
        vec![allow("ORDERS", ""), deny("*", "*PASSWORD*")],
    ));
    prepare_orders(&h);

    let mut args = query("ORDERS");
    args.filters = Some(vec![Filter {
        column: "password_hash".into(),
        op: FilterOp::Like,
        value: Some(json!("a%")),
    }]);
    denied_message(h.gateway.query_table(CLIENT, args));

    let mut args = query("ORDERS");
    args.order_by = Some(vec![OrderBy {
        column: "PASSWORD_HASH".into(),
        descending: false,
    }]);
    denied_message(h.gateway.query_table(CLIENT, args));
    assert!(h.db.seen.lock().unwrap().is_empty());
}

#[test]
fn a_table_whose_every_column_is_denied_is_refused() {
    let h = oracle();
    h.set_policy(data_policy(
        1,
        vec![allow("ORDERS", "ID"), deny("ORDERS", "ID")],
    ));
    prepare_orders(&h);
    denied_message(h.gateway.query_table(CLIENT, query("ORDERS")));
}

#[test]
fn unknown_columns_and_missing_tables_are_ordinary_errors() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("*", "")]));
    prepare_orders(&h);

    let mut args = query("ORDERS");
    args.columns = Some(vec!["nope".into()]);
    assert!(failed_message(h.gateway.query_table(CLIENT, args)).contains("Unknown column"));
    assert!(failed_message(h.gateway.query_table(CLIENT, query("GHOST"))).contains("not found"));
    assert_audit(&h, 2, McpOutcome::Error);
}

#[test]
fn identifiers_that_could_carry_sql_are_denied() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("*", "")]));
    prepare_orders(&h);
    for table in ["ORDERS; DROP TABLE X", "ORDERS\"", "a b", "ORDERS--", "x.y"] {
        denied_message(h.gateway.query_table(CLIENT, query(table)));
    }
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn filter_values_are_bound_not_spliced() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);
    let mut args = query("ORDERS");
    args.columns = Some(vec!["id".into()]);
    args.filters = Some(vec![Filter {
        column: "status".into(),
        op: FilterOp::Eq,
        value: Some(json!("x' OR '1'='1")),
    }]);
    h.gateway.query_table(CLIENT, args).unwrap();

    let seen = h.db.seen.lock().unwrap()[0].clone();
    assert!(!seen.sql.contains("OR '1'"));
    assert_eq!(seen.binds, ["x' OR '1'='1"]);
}

#[test]
fn the_row_limit_is_capped_by_policy_and_truncation_is_reported() {
    let h = oracle();
    h.set_policy(McpPolicy {
        max_rows: 5,
        ..data_policy(1, vec![allow("ORDERS", "")])
    });
    prepare_orders(&h);
    *h.db.readonly_result.lock().unwrap() = Some(rows(5, true));

    let mut args = query("ORDERS");
    args.limit = Some(1_000_000);
    let out = h.gateway.query_table(CLIENT, args).unwrap();
    assert_eq!(h.db.seen.lock().unwrap()[0].max_rows, 5);
    assert_eq!(out["truncated"], true);
    assert_eq!(out["row_limit"], 5);

    // A smaller request is honoured.
    *h.db.readonly_result.lock().unwrap() = Some(rows(2, false));
    let mut args = query("ORDERS");
    args.limit = Some(2);
    h.gateway.query_table(CLIENT, args).unwrap();
    assert_eq!(h.db.seen.lock().unwrap()[1].max_rows, 2);
}

#[test]
fn oversized_results_are_cut_off() {
    let h = oracle();
    h.set_policy(McpPolicy {
        max_rows: 10_000,
        ..data_policy(1, vec![allow("ORDERS", "")])
    });
    prepare_orders(&h);
    *h.db.readonly_result.lock().unwrap() = Some(ReadOnlyResult {
        columns: vec!["BIG".into()],
        rows: (0..400).map(|_| vec![Some("x".repeat(4_000))]).collect(),
        truncated: false,
    });
    let out = h.gateway.query_table(CLIENT, query("ORDERS")).unwrap();
    assert_eq!(out["truncated"], true);
    assert!(out["rows"].as_array().unwrap().len() < 400);
}

#[test]
fn postgres_allow_rules_are_case_sensitive_and_use_the_connection_schema() {
    let h = postgres();
    h.set_policy(data_policy(1, vec![allow("orders", "")]));
    h.db.columns
        .lock()
        .unwrap()
        .insert("ORDERS".into(), vec!["id".into(), "Status".into()]);
    *h.db.readonly_result.lock().unwrap() = Some(rows(1, false));

    denied_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    h.gateway.query_table(CLIENT, query("orders")).unwrap();
    let seen = h.db.seen.lock().unwrap()[0].clone();
    assert!(seen.sql.contains("FROM \"sales\".\"orders\""));
    assert!(seen.sql.contains("\"id\", \"Status\""));
}

#[test]
fn a_dedicated_account_is_used_for_data_reads_only() {
    let h = oracle();
    h.set_policy(McpPolicy {
        mcp_username: "svc_reader".into(),
        ..data_policy(1, vec![allow("ORDERS", "")])
    });
    prepare_orders(&h);

    // No password stored yet: refuse rather than fall back to the main account.
    let message = failed_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    assert!(message.contains("password is not stored"));
    assert!(h.db.seen.lock().unwrap().is_empty());

    h.policies
        .passwords
        .lock()
        .unwrap()
        .insert(1, "reader-pw".into());
    *h.db.readonly_result.lock().unwrap() = Some(rows(1, false));
    h.gateway.query_table(CLIENT, query("ORDERS")).unwrap();
    let seen = h.db.seen.lock().unwrap()[0].clone();
    assert_eq!(seen.username, "svc_reader");
    assert_eq!(seen.password, "reader-pw");
    // The table is still qualified with the connection's own schema.
    assert!(seen.sql.contains("\"APP\".\"ORDERS\""));
}

// ── run_query ───────────────────────────────────────────────────────────────────

fn raw_policy(rules: Vec<crate::models::McpTableRule>) -> McpPolicy {
    McpPolicy {
        allow_raw_sql: true,
        ..data_policy(1, rules)
    }
}

#[test]
fn raw_sql_is_off_unless_explicitly_enabled() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("*", "")]));
    let message = denied_message(h.gateway.run_query(CLIENT, raw("SELECT * FROM orders")));
    assert!(message.contains("allow_raw_sql"));
}

#[test]
fn allowed_selects_run_with_the_policy_limits() {
    let h = oracle();
    h.set_policy(McpPolicy {
        max_rows: 20,
        query_timeout_secs: 9,
        ..raw_policy(vec![allow("ORDERS", ""), allow("ITEMS", "")])
    });
    has_tables(&h, &["ORDERS", "ITEMS"]);
    *h.db.readonly_result.lock().unwrap() = Some(rows(1, false));

    let out = h
        .gateway
        .run_query(
            CLIENT,
            raw("SELECT o.id FROM orders o JOIN app.items i ON i.oid = o.id;"),
        )
        .unwrap();
    assert_eq!(out["row_count"], 1);
    let seen = h.db.seen.lock().unwrap()[0].clone();
    assert_eq!(seen.max_rows, 20);
    assert_eq!(seen.timeout_secs, 9);
    assert!(!seen.sql.ends_with(';'));
    assert_audit(&h, 1, McpOutcome::Allowed);
}

#[test]
fn raw_sql_touching_tables_outside_the_allowlist_is_denied() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("ORDERS", "")]));
    for sql in [
        "SELECT * FROM customers",
        "SELECT * FROM orders, customers",
        "SELECT * FROM orders WHERE id IN (SELECT id FROM customers)",
        "SELECT (SELECT max(x) FROM customers) FROM orders",
        "WITH c AS (SELECT * FROM customers) SELECT * FROM c",
        "SELECT * FROM orders UNION SELECT * FROM customers",
        "SELECT * FROM all_users",
    ] {
        denied_message(h.gateway.run_query(CLIENT, raw(sql)));
    }
    assert!(h.db.seen.lock().unwrap().is_empty());
}

#[test]
fn raw_sql_cannot_leave_the_connections_schema() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("*", "")]));
    let message = denied_message(
        h.gateway
            .run_query(CLIENT, raw("SELECT * FROM other.orders")),
    );
    assert!(message.contains("own schema"));
}

#[test]
fn raw_sql_is_refused_for_tables_with_column_rules() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("*", ""), deny("USERS", "PASSWORD")]));
    has_tables(&h, &["ORDERS", "USERS"]);
    let message = denied_message(h.gateway.run_query(CLIENT, raw("SELECT * FROM users")));
    assert!(message.contains("query_table"));
    // Other tables are fine.
    *h.db.readonly_result.lock().unwrap() = Some(rows(1, false));
    h.gateway
        .run_query(CLIENT, raw("SELECT * FROM orders"))
        .unwrap();
}

#[test]
fn writes_and_multiple_statements_are_denied_before_the_database() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("*", "")]));
    for sql in [
        "DELETE FROM orders",
        "UPDATE orders SET status = 'x'",
        "DROP TABLE orders",
        "SELECT 1 FROM orders; DROP TABLE orders",
        "WITH x AS (DELETE FROM orders RETURNING *) SELECT * FROM x",
        "SELECT * FROM orders FOR UPDATE",
        "SELECT dblink('x','y')",
    ] {
        denied_message(h.gateway.run_query(CLIENT, raw(sql)));
    }
    assert_eq!(h.db_calls(), 0);
    assert_audit(&h, 7, McpOutcome::Denied);
}

#[test]
fn raw_sql_limit_is_capped_by_policy() {
    let h = oracle();
    h.set_policy(McpPolicy {
        max_rows: 7,
        ..raw_policy(vec![allow("*", "")])
    });
    has_tables(&h, &["ORDERS"]);
    *h.db.readonly_result.lock().unwrap() = Some(rows(1, false));
    let mut args = raw("SELECT * FROM orders");
    args.max_rows = Some(99_999);
    h.gateway.run_query(CLIENT, args).unwrap();
    assert_eq!(h.db.seen.lock().unwrap()[0].max_rows, 7);
}

#[test]
fn raw_sql_cannot_read_system_catalogs_even_with_a_broad_allow_rule() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("*", "")]));
    has_tables(&h, &["ORDERS"]);
    for sql in [
        "SELECT * FROM pg_user",
        "SELECT * FROM all_users",
        "SELECT * FROM dba_tables",
        "SELECT * FROM v$session",
        "SELECT * FROM orders o JOIN all_tab_columns c ON 1=1",
    ] {
        let message = denied_message(h.gateway.run_query(CLIENT, raw(sql)));
        assert!(message.contains("not a table or view"), "query: {sql}");
    }
    assert!(h.db.seen.lock().unwrap().is_empty());
}

#[test]
fn raw_sql_forms_that_hide_a_table_from_a_naive_scan_are_denied() {
    let h = postgres();
    h.set_policy(raw_policy(vec![allow("orders", "")]));
    has_tables(&h, &["orders", "secret"]);
    for sql in [
        "SELECT 1 UNION TABLE secret",
        "WITH x AS (SELECT 1) TABLE secret",
        "SELECT * FROM (TABLE secret) s",
        "SELECT * FROM (secret CROSS JOIN orders) j",
        "SELECT * FROM ((secret CROSS JOIN orders) CROSS JOIN orders o2) j",
        "SELECT * FROM orders JOIN (secret JOIN orders o2 ON true) ON true",
        "SELECT * FROM (WITH secret AS (SELECT 1 AS a FROM orders) SELECT a FROM secret) s, secret",
    ] {
        assert!(
            matches!(
                h.gateway.run_query(CLIENT, raw(sql)),
                Err(ToolError::Denied(_))
            ),
            "should be denied: {sql}"
        );
    }
    assert!(h.db.seen.lock().unwrap().is_empty());
}

#[test]
fn oracle_national_alternative_quoting_cannot_desync_the_scanner() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("ORDERS", "")]));
    has_tables(&h, &["ORDERS", "SECRET"]);
    let sql = "SELECT nq'[a']' || (SELECT count(*) FROM secret) -- '\nFROM orders";
    assert!(matches!(
        h.gateway.run_query(CLIENT, raw(sql)),
        Err(ToolError::Denied(_))
    ));
}

#[test]
fn oversized_arguments_are_bounded_in_the_audit_log() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);
    let mut args = query("ORDERS");
    args.columns = Some((0..5_000).map(|i| format!("COLUMN_{i}")).collect());
    let _ = h.gateway.query_table(CLIENT, args);

    let entry = &h.entries()[0];
    assert!(entry.arguments.len() < 5_000, "{}", entry.arguments.len());
    let stored: Value = serde_json::from_str(&entry.arguments).unwrap();
    assert_eq!(stored["truncated"], true);
    assert_eq!(stored["sha256"].as_str().unwrap().len(), 64);

    let long_client = "x".repeat(10_000);
    let _ = h.gateway.list_connections(&long_client);
    assert!(h.entries()[1].client_name.chars().count() <= 201);
}
// ── save_migration_script ───────────────────────────────────────────────────────

fn save_args(name: &str, description: &str) -> SaveMigrationScriptArgs {
    SaveMigrationScriptArgs {
        connection_id: 1,
        object_name: name.into(),
        object_type: "TABLE".into(),
        ddl: Some("CREATE TABLE ORDERS (ID NUMBER);".into()),
        source: None,
        description: description.into(),
        dry_run: false,
    }
}

fn save_harness() -> (Harness, tempfile::TempDir) {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        ..enabled_policy(1)
    });
    let root = tempfile::tempdir().unwrap();
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());
    (h, root)
}

#[test]
fn saving_needs_the_policy_flag() {
    let h = oracle();
    h.set_policy(enabled_policy(1));
    let root = tempfile::tempdir().unwrap();
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, save_args("ORDERS", "add_col")),
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn saving_writes_code_and_migration_files_inside_the_schema_folder() {
    let (h, root) = save_harness();
    let out = h
        .gateway
        .save_migration_script(CLIENT, save_args("ORDERS", "add_status-col"))
        .unwrap();
    assert_eq!(out["written"], true);

    let files = out["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for file in files {
        let path = std::path::PathBuf::from(file["path"].as_str().unwrap());
        assert!(path.starts_with(root.path().join("app")), "{path:?}");
        assert!(path.is_file(), "{path:?}");
    }
    let code = std::fs::read_to_string(
        root.path()
            .join("app")
            .join("code")
            .join("app")
            .join("ORDERS.sql"),
    )
    .unwrap();
    assert_eq!(code, "CREATE TABLE ORDERS (ID NUMBER);");
    let migration = files.iter().find(|f| f["kind"] == "migration").unwrap();
    assert!(
        migration["path"]
            .as_str()
            .unwrap()
            .contains("add_status-col")
    );
    assert_audit(&h, 1, McpOutcome::Allowed);
}

#[test]
fn dry_run_returns_paths_and_content_without_writing() {
    let (h, root) = save_harness();
    let mut args = save_args("ORDERS", "preview");
    args.dry_run = true;
    let out = h.gateway.save_migration_script(CLIENT, args).unwrap();

    assert_eq!(out["dry_run"], true);
    assert_eq!(out["written"], false);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let code = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "code")
        .unwrap();
    assert_eq!(code["content"], "CREATE TABLE ORDERS (ID NUMBER);");
    assert_eq!(code["overwrites_existing"], false);
}

#[test]
fn saving_again_reports_the_overwrite() {
    let (h, _root) = save_harness();
    h.gateway
        .save_migration_script(CLIENT, save_args("ORDERS", "first"))
        .unwrap();
    let mut again = save_args("ORDERS", "second");
    again.dry_run = true;
    let out = h.gateway.save_migration_script(CLIENT, again).unwrap();
    let code = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "code")
        .unwrap();
    assert_eq!(code["overwrites_existing"], true);
}

#[test]
fn path_traversal_in_names_and_descriptions_is_rejected() {
    let (h, root) = save_harness();
    for (name, description) in [
        ("../evil", "ok"),
        ("..\\evil", "ok"),
        ("a/b", "ok"),
        ("a\\b", "ok"),
        ("..", "ok"),
        ("a.b", "ok"),
        ("C:\\x", "ok"),
        ("/etc/passwd", "ok"),
        ("", "ok"),
        ("ORDERS", "../../escape"),
        ("ORDERS", "a/b"),
        ("ORDERS", "a b"),
        ("ORDERS", ""),
        ("ORDERS", "with.dot"),
        ("ORDERS\0", "ok"),
    ] {
        let result = h
            .gateway
            .save_migration_script(CLIENT, save_args(name, description));
        assert!(
            matches!(result, Err(ToolError::Denied(_))),
            "expected a denial for name {name:?} / description {description:?}"
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn oversized_or_empty_ddl_is_rejected() {
    let (h, _root) = save_harness();
    let mut empty = save_args("ORDERS", "x");
    empty.ddl = Some("   ".into());
    denied_message(h.gateway.save_migration_script(CLIENT, empty));
    let mut huge = save_args("ORDERS", "x");
    huge.ddl = Some("x".repeat(1_000_001));
    denied_message(h.gateway.save_migration_script(CLIENT, huge));
}

#[test]
fn saving_without_a_configured_schema_folder_fails_cleanly() {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        ..enabled_policy(1)
    });
    let message = failed_message(
        h.gateway
            .save_migration_script(CLIENT, save_args("ORDERS", "x")),
    );
    assert!(message.contains("Schema folder"));
}

#[test]
fn hidden_objects_cannot_have_scripts_saved() {
    let (h, root) = save_harness();
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "ORDERS".into(),
        }],
        ..enabled_policy(1)
    });
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, save_args("ORDERS", "x")),
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn the_audit_trail_keeps_a_digest_of_saved_ddl_not_the_ddl_itself() {
    let (h, _root) = save_harness();
    let mut args = save_args("ORDERS", "x");
    args.ddl = Some("CREATE TABLE ORDERS (SECRET_COLUMN_NAME NUMBER);".into());
    h.gateway.save_migration_script(CLIENT, args).unwrap();

    let entry = &h.entries()[0];
    assert!(!entry.arguments.contains("SECRET_COLUMN_NAME"));
    let arguments: Value = serde_json::from_str(&entry.arguments).unwrap();
    assert_eq!(arguments["ddl_bytes"], 48);
    assert_eq!(arguments["ddl_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(arguments["target_paths"].as_array().unwrap().len(), 2);
}

// ── save_migration_script with a source ─────────────────────────────────────────

fn source_args(source: SaveSource) -> SaveMigrationScriptArgs {
    SaveMigrationScriptArgs {
        ddl: None,
        source: Some(source),
        ..save_args("ORDERS", "from_source")
    }
}

fn from_file(path: &str) -> SaveMigrationScriptArgs {
    source_args(SaveSource {
        server: None,
        file: Some(path.into()),
    })
}

fn from_server() -> SaveMigrationScriptArgs {
    source_args(SaveSource {
        server: Some(true),
        file: None,
    })
}

/// Writes `bytes` to `<schema folder>/<relative>` and returns the schema folder.
fn put_source(root: &tempfile::TempDir, relative: &str, bytes: &[u8]) -> std::path::PathBuf {
    let schema_dir = root.path().join("app");
    let file = schema_dir.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, bytes).unwrap();
    schema_dir
}

fn saved_file<'a>(out: &'a Value, kind: &str) -> &'a Value {
    out["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == kind)
        .unwrap()
}

#[test]
fn server_source_saves_the_connections_ddl_without_echoing_it() {
    let (h, _root) = save_harness();
    let ddl = "CREATE TABLE ORDERS (SECRET_COLUMN_NAME NUMBER);\n";
    h.db.ddls
        .lock()
        .unwrap()
        .insert("TABLE:ORDERS".into(), ddl.into());

    let out = h
        .gateway
        .save_migration_script(CLIENT, from_server())
        .unwrap();
    assert_eq!(out["written"], true);
    assert_eq!(out["source"]["kind"], "server");
    assert_eq!(out["source"]["bytes"], ddl.len());
    assert!(!out.to_string().contains("SECRET_COLUMN_NAME"));

    let code = saved_file(&out, "code");
    let on_disk = std::fs::read(code["path"].as_str().unwrap()).unwrap();
    assert_eq!(on_disk, ddl.as_bytes());
    assert_eq!(code["bytes"], ddl.len());
    assert_eq!(code["sha256"], sha256_hex(ddl.as_bytes()));
    assert_eq!(code["line_count"], 1);
    assert_eq!(code["overwrites_existing"], false);
    assert!(
        std::path::Path::new(saved_file(&out, "migration")["path"].as_str().unwrap()).is_file()
    );

    assert_audit(&h, 1, McpOutcome::Allowed);
    let entry = &h.entries()[0];
    assert!(!entry.arguments.contains("SECRET_COLUMN_NAME"));
    let arguments: Value = serde_json::from_str(&entry.arguments).unwrap();
    assert_eq!(arguments["source_kind"], "server");
    assert_eq!(arguments["source_bytes"], ddl.len());
    assert_eq!(arguments["source_sha256"], sha256_hex(ddl.as_bytes()));
}

#[test]
fn server_source_dry_run_reports_metadata_only() {
    let (h, root) = save_harness();
    h.db.ddls.lock().unwrap().insert(
        "TABLE:ORDERS".into(),
        "CREATE TABLE ORDERS (ID NUMBER);".into(),
    );
    let mut args = from_server();
    args.dry_run = true;
    let out = h.gateway.save_migration_script(CLIENT, args).unwrap();

    assert_eq!(out["written"], false);
    assert!(saved_file(&out, "code").get("content").is_none());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn server_source_needs_ddl_read_permission_and_respects_hidden_objects() {
    let (h, root) = save_harness();
    h.db.ddls.lock().unwrap().insert(
        "TABLE:ORDERS".into(),
        "CREATE TABLE ORDERS (ID NUMBER);".into(),
    );
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        allow_ddl_read: false,
        ..enabled_policy(1)
    });
    let message = denied_message(h.gateway.save_migration_script(CLIENT, from_server()));
    assert!(message.contains("allow_ddl_read"));
    assert_eq!(*h.db.db_calls.lock().unwrap(), 0);

    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "ORDERS".into(),
        }],
        ..enabled_policy(1)
    });
    denied_message(h.gateway.save_migration_script(CLIENT, from_server()));
    assert_eq!(*h.db.db_calls.lock().unwrap(), 0);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert_audit(&h, 2, McpOutcome::Denied);
}

#[test]
fn server_source_without_ddl_fails_and_is_audited() {
    let (h, root) = save_harness();
    failed_message(h.gateway.save_migration_script(CLIENT, from_server()));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    assert_audit(&h, 1, McpOutcome::Error);
}

#[test]
fn file_source_is_copied_byte_for_byte_into_every_file() {
    let (h, root) = save_harness();
    // BOM, CRLF line endings, a Windows-1257 byte that is not valid UTF-8, no final newline.
    let bytes: &[u8] = b"\xEF\xBB\xBFCREATE TABLE ORDERS (\r\n  NAME VARCHAR2(10) -- \xE0\r\n);";
    put_source(&root, "src/orders.sql", bytes);

    let out = h
        .gateway
        .save_migration_script(CLIENT, from_file("src/orders.sql"))
        .unwrap();
    assert_eq!(out["source"]["kind"], "file");
    assert_eq!(out["source"]["sha256"], sha256_hex(bytes));
    assert!(!out.to_string().contains("CREATE TABLE"));

    for kind in ["code", "migration"] {
        let file = saved_file(&out, kind);
        assert_eq!(
            std::fs::read(file["path"].as_str().unwrap()).unwrap(),
            bytes,
            "{kind}"
        );
        assert_eq!(file["bytes"], bytes.len());
        assert_eq!(file["sha256"], sha256_hex(bytes));
        assert_eq!(file["line_count"], 3);
        assert_eq!(file["overwrites_existing"], false);
    }

    let entry = &h.entries()[0];
    assert!(!entry.arguments.contains("CREATE TABLE"));
    let arguments: Value = serde_json::from_str(&entry.arguments).unwrap();
    assert_eq!(arguments["source_kind"], "file");
    assert_eq!(arguments["source_bytes"], bytes.len());
    assert_eq!(arguments["source_sha256"], sha256_hex(bytes));
    assert_audit(&h, 1, McpOutcome::Allowed);
}

#[test]
fn file_source_dry_run_never_returns_content_and_reports_overwrites() {
    let (h, root) = save_harness();
    put_source(&root, "orders.sql", b"CREATE TABLE ORDERS (ID NUMBER);\n");
    h.gateway
        .save_migration_script(CLIENT, from_file("orders.sql"))
        .unwrap();

    let mut args = from_file("./orders.sql");
    args.dry_run = true;
    args.description = "again".into();
    let out = h.gateway.save_migration_script(CLIENT, args).unwrap();
    assert_eq!(out["written"], false);
    let code = saved_file(&out, "code");
    assert!(code.get("content").is_none());
    assert_eq!(code["overwrites_existing"], true);
    assert_eq!(code["line_count"], 1);
}

#[test]
fn file_source_may_be_the_file_it_overwrites() {
    let (h, root) = save_harness();
    let bytes = b"CREATE TABLE ORDERS (ID NUMBER);\n";
    put_source(&root, "code/app/ORDERS.sql", bytes);
    let out = h
        .gateway
        .save_migration_script(CLIENT, from_file("code/app/ORDERS.sql"))
        .unwrap();
    assert_eq!(saved_file(&out, "code")["overwrites_existing"], true);
    let code = std::fs::read(saved_file(&out, "code")["path"].as_str().unwrap()).unwrap();
    assert_eq!(code, bytes);
}

#[test]
fn file_source_paths_cannot_leave_the_schema_folder() {
    let (h, root) = save_harness();
    put_source(&root, "ok.sql", b"SELECT 1 FROM DUAL;");
    std::fs::write(root.path().join("outside.sql"), b"SELECT 2 FROM DUAL;").unwrap();
    let outside_abs = root
        .path()
        .join("outside.sql")
        .to_string_lossy()
        .into_owned();

    let mut calls = 0;
    for path in [
        "../outside.sql",
        "sub/../../outside.sql",
        "..\\outside.sql",
        outside_abs.as_str(),
        "/etc/passwd",
        "\\\\server\\share\\x.sql",
        "C:\\Windows\\win.ini",
        "C:win.ini",
        "ok.sql:stream",
        "",
        "   ",
        "ok\0.sql",
    ] {
        calls += 1;
        let message = denied_message(h.gateway.save_migration_script(CLIENT, from_file(path)));
        assert!(!message.is_empty(), "{path:?}");
        assert_audit(&h, calls, McpOutcome::Denied);
    }
    // Nothing was written next to the sources.
    assert_eq!(
        std::fs::read_dir(root.path().join("app")).unwrap().count(),
        1
    );
}

#[test]
fn file_source_symlinks_cannot_escape_the_schema_folder() {
    let (h, root) = save_harness();
    let schema_dir = put_source(&root, "ok.sql", b"SELECT 1 FROM DUAL;");
    let outside = root.path().join("outside.sql");
    std::fs::write(&outside, b"SELECT 2 FROM DUAL;").unwrap();
    let outside_dir = root.path().join("elsewhere");
    std::fs::create_dir_all(&outside_dir).unwrap();
    std::fs::write(outside_dir.join("x.sql"), b"SELECT 3 FROM DUAL;").unwrap();

    #[cfg(unix)]
    let linked = {
        use std::os::unix::fs::symlink;
        symlink(&outside, schema_dir.join("link.sql")).is_ok()
            && symlink(&outside_dir, schema_dir.join("linkdir")).is_ok()
    };
    #[cfg(windows)]
    let linked = {
        // Directory junctions need no privilege, unlike symlinks.
        let link = schema_dir.join("linkdir");
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&outside_dir)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    };
    if !linked {
        eprintln!("skipping: symlinks cannot be created here");
        return;
    }

    #[cfg(unix)]
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("link.sql")),
    );
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("linkdir/x.sql")),
    );
    assert_audit(&h, if cfg!(unix) { 2 } else { 1 }, McpOutcome::Denied);
}

#[test]
fn file_source_must_be_an_existing_non_empty_text_file() {
    let (h, root) = save_harness();
    put_source(&root, "empty.sql", b"");
    put_source(&root, "blank.sql", b" \r\n\t");
    put_source(&root, "binary.sql", b"CREATE\0TABLE");
    put_source(&root, "dir/inner.sql", b"SELECT 1 FROM DUAL;");

    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("empty.sql")),
    );
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("blank.sql")),
    );
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("binary.sql")),
    );
    denied_message(h.gateway.save_migration_script(CLIENT, from_file("dir")));
    failed_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("missing.sql")),
    );
    assert_eq!(h.entries().len(), 5);
}

#[test]
fn file_source_size_cap_is_eight_megabytes() {
    let (h, root) = save_harness();
    // Over the 1 MB inline limit, so only the file path can carry it.
    let big = vec![b'x'; 2_000_000];
    put_source(&root, "big.sql", &big);
    let out = h
        .gateway
        .save_migration_script(CLIENT, from_file("big.sql"))
        .unwrap();
    assert_eq!(saved_file(&out, "code")["bytes"], big.len());

    put_source(&root, "huge.sql", &vec![b'x'; 8 * 1024 * 1024 + 1]);
    let message = denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("huge.sql")),
    );
    assert!(message.contains("8 MB"));
}

#[test]
fn exactly_one_of_ddl_and_source_is_required() {
    let (h, root) = save_harness();
    put_source(&root, "orders.sql", b"SELECT 1 FROM DUAL;");

    let mut neither = save_args("ORDERS", "x");
    neither.ddl = None;
    assert!(
        denied_message(h.gateway.save_migration_script(CLIENT, neither)).contains("exactly one")
    );

    let mut both = from_file("orders.sql");
    both.ddl = Some("CREATE TABLE ORDERS (ID NUMBER);".into());
    assert!(denied_message(h.gateway.save_migration_script(CLIENT, both)).contains("exactly one"));

    for source in [
        SaveSource {
            server: None,
            file: None,
        },
        SaveSource {
            server: Some(false),
            file: None,
        },
        SaveSource {
            server: Some(true),
            file: Some("orders.sql".into()),
        },
    ] {
        denied_message(h.gateway.save_migration_script(CLIENT, source_args(source)));
    }
    assert_audit(&h, 5, McpOutcome::Denied);
    assert_eq!(
        std::fs::read_dir(root.path().join("app")).unwrap().count(),
        1
    );
}

#[test]
fn file_source_cannot_save_hidden_objects_or_without_the_flag() {
    let (h, root) = save_harness();
    put_source(&root, "orders.sql", b"SELECT 1 FROM DUAL;");
    h.set_policy(McpPolicy {
        allow_save_scripts: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "ORDERS".into(),
        }],
        ..enabled_policy(1)
    });
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("orders.sql")),
    );
    h.set_policy(enabled_policy(1));
    denied_message(
        h.gateway
            .save_migration_script(CLIENT, from_file("orders.sql")),
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("app")).unwrap().count(),
        1
    );
}

#[test]
fn source_arguments_deserialize_and_reject_unknown_fields() {
    let args: SaveMigrationScriptArgs = serde_json::from_value(json!({
        "connection_id": 1,
        "object_name": "ORDERS",
        "object_type": "TABLE",
        "description": "x",
        "source": { "file": "a/b.sql" },
    }))
    .unwrap();
    assert!(args.ddl.is_none());
    assert_eq!(args.source.unwrap().file.as_deref(), Some("a/b.sql"));

    let typo = serde_json::from_value::<SaveMigrationScriptArgs>(json!({
        "connection_id": 1,
        "object_name": "ORDERS",
        "object_type": "TABLE",
        "description": "x",
        "source": { "files": "a/b.sql" },
    }));
    assert!(typo.is_err());
}

#[test]
fn line_count_counts_a_final_unterminated_line() {
    assert_eq!(line_count(b""), 0);
    assert_eq!(line_count(b"a"), 1);
    assert_eq!(line_count(b"a\n"), 1);
    assert_eq!(line_count(b"a\nb"), 2);
    assert_eq!(line_count(b"a\r\nb\r\n"), 2);
    assert_eq!(line_count(b"\n\n"), 2);
}

// ── compare / fix ───────────────────────────────────────────────────────────────

fn table(columns: &[&str]) -> crate::models::TableColumns {
    columns
        .iter()
        .map(|c| {
            (
                c.to_string(),
                ColumnInfo {
                    data_type: Some("NUMBER".into()),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn compare_harness() -> Harness {
    let h = Harness::new(vec![
        connection(1, "Source", DbType::Oracle),
        connection(2, "Target", DbType::Oracle),
    ]);
    for id in [1, 2] {
        h.set_policy(McpPolicy {
            allow_compare_fix: true,
            ..enabled_policy(id)
        });
    }
    let mut source = crate::models::ServerTables::new();
    source.insert("ORDERS".into(), table(&["ID", "STATUS"]));
    source.insert("AUDIT_LOG".into(), table(&["ID"]));
    let mut target = crate::models::ServerTables::new();
    target.insert("ORDERS".into(), table(&["ID"]));
    h.db.schemas.lock().unwrap().insert(1, source);
    h.db.schemas.lock().unwrap().insert(2, target);
    h.db.table_ddls.lock().unwrap().insert(
        "AUDIT_LOG".into(),
        "CREATE TABLE AUDIT_LOG (ID NUMBER);".into(),
    );
    h
}

fn compare_args() -> CompareSchemasArgs {
    CompareSchemasArgs {
        source_id: 1,
        target_ids: vec![2],
        check_tables: None,
        check_columns: None,
        check_comments: None,
        check_indexes: None,
    }
}

#[test]
fn compare_reports_discrepancies_between_connections() {
    let h = compare_harness();
    let out = h.gateway.compare_schemas(CLIENT, compare_args()).unwrap();

    assert_eq!(out["count"], 2);
    let tables: Vec<&str> = out["discrepancies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["table"].as_str().unwrap())
        .collect();
    assert!(tables.contains(&"ORDERS") && tables.contains(&"AUDIT_LOG"));
    assert_audit(&h, 1, McpOutcome::Allowed);
}

#[test]
fn compare_needs_the_permission_on_every_involved_connection() {
    let h = compare_harness();
    h.set_policy(enabled_policy(2));
    let message = denied_message(h.gateway.compare_schemas(CLIENT, compare_args()));
    assert!(message.contains("allow_compare_fix"));
    assert_eq!(h.db_calls(), 0);
    assert!(matches!(
        h.gateway.generate_fix_script(
            CLIENT,
            GenerateFixScriptArgs {
                source_id: 1,
                target_ids: vec![2],
                tables: None,
                check_tables: None,
                check_columns: None,
                check_comments: None,
                check_indexes: None,
            }
        ),
        Err(ToolError::Denied(_))
    ));
}

#[test]
fn compare_refuses_to_mix_engines() {
    let h = Harness::new(vec![
        connection(1, "Ora", DbType::Oracle),
        connection(2, "Pg", DbType::Postgres),
    ]);
    for id in [1, 2] {
        h.set_policy(McpPolicy {
            allow_compare_fix: true,
            ..enabled_policy(id)
        });
    }
    let message = failed_message(h.gateway.compare_schemas(CLIENT, compare_args()));
    assert!(message.contains("different database engines"));
}

#[test]
fn compare_hides_discrepancies_for_denied_objects() {
    let h = compare_harness();
    h.set_policy(McpPolicy {
        allow_compare_fix: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "TABLE".into(),
            name: "AUDIT_*".into(),
        }],
        ..enabled_policy(2)
    });
    let out = h.gateway.compare_schemas(CLIENT, compare_args()).unwrap();
    assert_eq!(out["count"], 1);
    assert_eq!(out["hidden_by_policy"], 1);
    assert!(!out.to_string().contains("AUDIT_LOG"));
}

#[test]
fn fix_scripts_are_generated_never_executed_and_respect_hidden_objects() {
    let h = compare_harness();
    let out = h
        .gateway
        .generate_fix_script(
            CLIENT,
            GenerateFixScriptArgs {
                source_id: 1,
                target_ids: vec![2],
                tables: None,
                check_tables: None,
                check_columns: None,
                check_comments: None,
                check_indexes: None,
            },
        )
        .unwrap();
    let script = out["script"].as_str().unwrap();
    assert!(script.contains("AUDIT_LOG"));
    assert!(script.to_uppercase().contains("STATUS"));
    assert!(out["generated_count"].as_u64().unwrap() >= 2);

    // Restrict to one table.
    *h.db.table_ddls.lock().unwrap() = Default::default();
    let only = h
        .gateway
        .generate_fix_script(
            CLIENT,
            GenerateFixScriptArgs {
                source_id: 1,
                target_ids: vec![2],
                tables: Some(vec!["orders".into()]),
                check_tables: None,
                check_columns: None,
                check_comments: None,
                check_indexes: None,
            },
        )
        .unwrap();
    assert!(!only["script"].as_str().unwrap().contains("AUDIT_LOG"));

    // Nothing selected is an error, not an empty script.
    let none = h.gateway.generate_fix_script(
        CLIENT,
        GenerateFixScriptArgs {
            source_id: 1,
            target_ids: vec![2],
            tables: Some(vec!["NOPE".into()]),
            check_tables: None,
            check_columns: None,
            check_comments: None,
            check_indexes: None,
        },
    );
    assert!(matches!(none, Err(ToolError::Failed(_))));
}

#[test]
fn compare_needs_at_least_one_target() {
    let h = compare_harness();
    let mut args = compare_args();
    args.target_ids.clear();
    denied_message(h.gateway.compare_schemas(CLIENT, args));
}

// ── audit invariants ────────────────────────────────────────────────────────────

#[test]
fn every_call_produces_exactly_one_entry_whatever_its_outcome() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);
    *h.db.objects.lock().unwrap() = vec![SchemaObject {
        name: "ORDERS".into(),
        object_type: "TABLE".into(),
    }];

    let _ = h.gateway.list_connections(CLIENT); // allowed
    let _ = h.gateway.query_table(CLIENT, query("ORDERS")); // allowed
    let _ = h.gateway.query_table(CLIENT, query("CUSTOMERS")); // denied
    let _ = h.gateway.query_table(CLIENT, query("GHOST")); // error (not allowed → denied)
    let _ = h.gateway.run_query(CLIENT, raw("SELECT 1 FROM DUAL")); // denied (raw off)
    let _ = h.gateway.list_schema_objects(
        CLIENT,
        ListSchemaObjectsArgs {
            connection_id: 42,
            object_type: None,
        },
    ); // denied (unknown)

    let entries = h.entries();
    assert_eq!(entries.len(), 6);
    let outcomes: Vec<McpOutcome> = entries.iter().map(|e| e.outcome).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == McpOutcome::Allowed)
            .count(),
        2
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| **o == McpOutcome::Denied)
            .count(),
        4
    );
    // The chain they form is intact.
    assert!(
        crate::services::mcp_audit::McpAuditService::new(h.audit.clone())
            .without_forwarding()
            .verify()
            .unwrap()
            .ok
    );
}

#[test]
fn entries_record_client_connection_arguments_and_denial_reasons() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));

    let _ = h.gateway.query_table("claude-code 2.1", query("CUSTOMERS"));
    let entry = &h.entries()[0];
    assert_eq!(entry.client_name, "claude-code 2.1");
    assert_eq!(entry.tool, "query_table");
    assert_eq!(entry.connection_id, Some(1));
    assert_eq!(entry.connection_name, "Prod");
    assert_eq!(entry.outcome, McpOutcome::Denied);
    assert!(entry.reason.contains("CUSTOMERS"));
    assert!(entry.arguments.contains("CUSTOMERS"));
    assert!(entry.duration_ms >= 0);
}

#[test]
fn raw_sql_text_is_audited_even_when_denied() {
    let h = oracle();
    h.set_policy(raw_policy(vec![allow("ORDERS", "")]));
    let _ = h.gateway.run_query(CLIENT, raw("SELECT * FROM customers"));
    let entry = &h.entries()[0];
    assert_eq!(entry.outcome, McpOutcome::Denied);
    assert!(entry.arguments.contains("SELECT * FROM customers"));
}

#[test]
fn credentials_never_reach_the_audit_trail_or_results() {
    let h = oracle();
    h.set_policy(McpPolicy {
        mcp_username: "svc_reader".into(),
        allow_save_scripts: true,
        allow_compare_fix: true,
        allow_raw_sql: true,
        ..data_policy(1, vec![allow("ORDERS", "")])
    });
    h.policies
        .passwords
        .lock()
        .unwrap()
        .insert(1, "reader-pw".into());
    prepare_orders(&h);

    let mut outputs = vec![h.gateway.list_connections(CLIENT)];
    outputs.push(h.gateway.query_table(CLIENT, query("ORDERS")));
    outputs.push(h.gateway.run_query(CLIENT, raw("SELECT * FROM customers")));
    for output in outputs {
        let text = match output {
            Ok(v) => v.to_string(),
            Err(e) => e.to_string(),
        };
        for secret in [PASSWORD, "reader-pw"] {
            assert!(!text.contains(secret));
        }
    }
    for entry in h.entries() {
        let text = format!("{entry:?}");
        for secret in [PASSWORD, "reader-pw"] {
            assert!(!text.contains(secret), "a sensitive value leaked");
        }
    }
}

#[test]
fn when_the_audit_log_cannot_be_written_the_result_is_withheld() {
    let h = oracle();
    h.set_policy(data_policy(1, vec![allow("ORDERS", "")]));
    prepare_orders(&h);
    h.audit.fail_appends.store(true, Ordering::SeqCst);

    let message = failed_message(h.gateway.query_table(CLIENT, query("ORDERS")));
    assert!(message.contains("audit log could not be written"));
    assert!(!message.contains("OPEN"));

    // Denials fail closed too: the agent learns nothing beyond the audit failure.
    let message = failed_message(h.gateway.query_table(CLIENT, query("CUSTOMERS")));
    assert!(message.contains("audit log could not be written"));
}

// ── compare_object ──────────────────────────────────────────────────────────────

const SERVER_VIEW_DDL: &str = "\n  CREATE OR REPLACE FORCE EDITIONABLE VIEW \"APP\".\"ORDERS_V\" (\"ID\") AS \n  SELECT id FROM orders\n";

fn compare_object_args(file_path: &str) -> CompareObjectArgs {
    CompareObjectArgs {
        connection_id: 1,
        object_name: "ORDERS_V".into(),
        object_type: "VIEW".into(),
        file_path: file_path.into(),
    }
}

/// An Oracle connection with compare enabled, a schema folder and the server DDL of
/// `ORDERS_V` (which the fake database reports as `ddl`).
fn compare_object_harness(ddl: &str) -> (Harness, tempfile::TempDir) {
    let h = oracle();
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        ..enabled_policy(1)
    });
    h.db.ddls
        .lock()
        .unwrap()
        .insert("VIEW:ORDERS_V".into(), ddl.into());
    let root = tempfile::tempdir().unwrap();
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());
    std::fs::create_dir_all(root.path().join("app").join("code")).unwrap();
    (h, root)
}

fn write_schema_file(root: &tempfile::TempDir, relative: &str, bytes: &[u8]) {
    std::fs::write(root.path().join("app").join(relative), bytes).unwrap();
}

#[test]
fn compare_object_is_off_by_default_and_needs_ddl_read() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(&root, "code/v.sql", b"x");

    h.set_policy(enabled_policy(1));
    denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql")),
    );
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        allow_ddl_read: false,
        ..enabled_policy(1)
    });
    denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql")),
    );

    assert_eq!(h.db_calls(), 0);
    assert_eq!(h.entries().len(), 2);
    assert!(h.entries().iter().all(|e| e.tool == "compare_object"));
    assert!(!McpPolicy::default().allow_compare_object);
}

#[test]
fn identical_files_report_identical_despite_cosmetic_noise() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(
        &root,
        "code/v.sql",
        b"create or replace force view orders_v (\"ID\") AS\r\n  SELECT id FROM orders  \r\n\r\n",
    );
    let out = h
        .gateway
        .compare_object(CLIENT, compare_object_args("code/v.sql"))
        .unwrap();
    assert_eq!(out, json!({ "identical": true }));

    let entries = h.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].tool, "compare_object");
    assert_eq!(entries[0].outcome, McpOutcome::Allowed);
    let arguments: Value = serde_json::from_str(&entries[0].arguments).unwrap();
    assert_eq!(arguments["object_name"], "ORDERS_V");
    assert_eq!(arguments["file_path"], "code/v.sql");
    assert_eq!(arguments["result"], "identical");
}

#[test]
fn differences_come_back_as_a_bounded_diff_and_the_audit_keeps_only_counts() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(
        &root,
        "code/v.sql",
        b"CREATE FORCE VIEW ORDERS_V (\"ID\") AS\n  SELECT id, SECRET_FILE_COLUMN FROM orders\n",
    );
    let out = h
        .gateway
        .compare_object(CLIENT, compare_object_args("code/v.sql"))
        .unwrap();
    assert_eq!(out["identical"], false);
    assert_eq!(
        (out["added"].as_u64(), out["removed"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(out["hunks"], 1);
    assert_eq!(out["truncated"], false);
    let diff = out["diff"].as_str().unwrap();
    assert!(diff.contains("-  SELECT id FROM orders"), "{diff}");
    assert!(
        diff.contains("+  SELECT id, SECRET_FILE_COLUMN FROM orders"),
        "{diff}"
    );

    let entries = h.entries();
    assert_eq!(entries.len(), 1);
    let arguments: Value = serde_json::from_str(&entries[0].arguments).unwrap();
    assert_eq!(arguments["result"], "different");
    assert_eq!(arguments["diff_added"], 1);
    assert_eq!(arguments["diff_removed"], 1);
    assert!(arguments["diff_lines"].as_u64().unwrap() > 0);
    let logged = format!("{:?}", entries[0]);
    assert!(!logged.contains("SECRET_FILE_COLUMN"));
    assert!(!logged.contains("SELECT id FROM orders"));
}

#[test]
fn compare_object_refuses_paths_that_leave_the_schema_folder() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    std::fs::write(root.path().join("outside.sql"), "secret").unwrap();
    write_schema_file(&root, "code/v.sql", b"x");
    let absolute = root
        .path()
        .join("outside.sql")
        .to_string_lossy()
        .into_owned();

    let bad = [
        "../outside.sql".to_string(),
        "code/../../outside.sql".to_string(),
        "code\\..\\..\\outside.sql".to_string(),
        absolute,
        "/etc/passwd".to_string(),
        "C:\\Windows\\win.ini".to_string(),
        String::new(),
    ];
    for path in &bad {
        let message = denied_message(h.gateway.compare_object(CLIENT, compare_object_args(path)));
        assert!(!message.contains("secret"));
    }
    assert_eq!(h.db_calls(), 0);
    assert_eq!(h.entries().len(), bad.len());
    assert!(h.entries().iter().all(|e| e.outcome == McpOutcome::Denied));
}

#[cfg(unix)]
#[test]
fn compare_object_refuses_symlinks_that_escape_the_schema_folder() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    let outside = root.path().join("outside.sql");
    std::fs::write(&outside, "secret").unwrap();
    std::os::unix::fs::symlink(&outside, root.path().join("app").join("link.sql")).unwrap();
    denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("link.sql")),
    );
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn compare_object_respects_hidden_objects_and_validates_names() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(&root, "code/v.sql", b"x");
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "VIEW".into(),
            name: "ORDERS_*".into(),
        }],
        ..enabled_policy(1)
    });
    let message = denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql")),
    );
    assert!(message.contains("hidden by policy"));

    let mut args = compare_object_args("code/v.sql");
    args.object_name = "ORDERS_V; DROP".into();
    denied_message(h.gateway.compare_object(CLIENT, args));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn compare_object_caps_the_file_size_and_reports_missing_files() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(&root, "code/big.sql", &vec![b'x'; 1_000_001]);
    let message = denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/big.sql")),
    );
    assert!(message.contains("larger than"));

    let message = failed_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/none.sql")),
    );
    assert!(message.contains("not found"));
    assert_eq!(h.db_calls(), 0);
    assert_eq!(h.entries().len(), 2);
}

#[test]
fn compare_object_without_a_schema_folder_fails_cleanly() {
    let (h, _root) = compare_object_harness(SERVER_VIEW_DDL);
    *h.settings.schema_root.lock().unwrap() = None;
    failed_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql")),
    );
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn compare_object_decodes_the_file_with_the_configured_encoding() {
    let ddl = "CREATE VIEW ORDERS_V AS SELECT 'ā' FROM dual";
    let (h, root) = compare_object_harness(ddl);
    // "ā" is 0xE2 in Windows-1257.
    let mut bytes = b"CREATE VIEW ORDERS_V AS SELECT '".to_vec();
    bytes.push(0xE2);
    bytes.extend_from_slice(b"' FROM dual\r\n");
    write_schema_file(&root, "code/v.sql", &bytes);

    // Read as UTF-8 the file is invalid.
    failed_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql")),
    );

    h.settings.options.lock().unwrap().encoding = "windows-1257".into();
    let out = h
        .gateway
        .compare_object(CLIENT, compare_object_args("code/v.sql"))
        .unwrap();
    assert_eq!(out, json!({ "identical": true }));

    // A BOM-prefixed UTF-8 file matches too.
    h.settings.options.lock().unwrap().encoding = "utf8-bom".into();
    let mut bom = b"\xEF\xBB\xBF".to_vec();
    bom.extend_from_slice(ddl.as_bytes());
    write_schema_file(&root, "code/v.sql", &bom);
    assert_eq!(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/v.sql"))
            .unwrap(),
        json!({ "identical": true })
    );
}

#[test]
fn compare_object_uses_the_connections_schema_folder_on_postgres() {
    let h = postgres();
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        ..enabled_policy(1)
    });
    h.db.ddls.lock().unwrap().insert(
        "VIEW:ORDERS_V".into(),
        "CREATE OR REPLACE VIEW sales.orders_v AS SELECT 1;\n".into(),
    );
    let root = tempfile::tempdir().unwrap();
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());
    std::fs::create_dir_all(root.path().join("sales")).unwrap();
    std::fs::write(
        root.path().join("sales").join("v.sql"),
        "CREATE OR REPLACE VIEW sales.orders_v AS SELECT 1\r\n",
    )
    .unwrap();
    assert_eq!(
        h.gateway
            .compare_object(CLIENT, compare_object_args("v.sql"))
            .unwrap(),
        json!({ "identical": true })
    );

    // Postgres headers are not normalised: dropping OR REPLACE is a real difference.
    std::fs::write(
        root.path().join("sales").join("v.sql"),
        "CREATE VIEW sales.orders_v AS SELECT 1\n",
    )
    .unwrap();
    let out = h
        .gateway
        .compare_object(CLIENT, compare_object_args("v.sql"))
        .unwrap();
    assert_eq!(out["identical"], false);
}

#[test]
fn compare_object_reports_truncation_of_long_diffs() {
    let server: String = (0..600).map(|i| format!("server line {i}\n")).collect();
    let (h, root) = compare_object_harness(&server);
    let file: String = (0..600).map(|i| format!("file line {i}\n")).collect();
    write_schema_file(&root, "code/v.sql", file.as_bytes());
    let out = h
        .gateway
        .compare_object(CLIENT, compare_object_args("code/v.sql"))
        .unwrap();
    assert_eq!(out["truncated"], true);
    assert_eq!(out["diff_lines"], 400);
    assert_eq!(
        (out["added"].as_u64(), out["removed"].as_u64()),
        (Some(600), Some(600))
    );
}

#[test]
fn compare_object_on_a_missing_server_object_is_an_audited_error() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    write_schema_file(&root, "code/v.sql", b"x");
    let mut args = compare_object_args("code/v.sql");
    args.object_name = "GHOST".into();
    failed_message(h.gateway.compare_object(CLIENT, args));
    assert_audit(&h, 1, McpOutcome::Error);
}

#[test]
fn a_file_named_after_a_hidden_object_cannot_be_compared() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "SECRET_PKG".into(),
        }],
        ..enabled_policy(1)
    });
    write_schema_file(&root, "code/SECRET_PKG.sql", b"CREATE VIEW x AS SELECT 1");
    write_schema_file(
        &root,
        "code/260101_1200_app$SECRET_PKG.sql",
        b"CREATE VIEW x AS SELECT 1",
    );
    for file in ["code/SECRET_PKG.sql", "code/260101_1200_app$SECRET_PKG.sql"] {
        let message = denied_message(h.gateway.compare_object(CLIENT, compare_object_args(file)));
        assert!(message.contains("hidden"), "{message}");
    }
}

#[test]
fn a_file_that_mentions_a_hidden_object_cannot_be_compared() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "*".into(),
            name: "SECRET_PKG".into(),
        }],
        ..enabled_policy(1)
    });
    write_schema_file(
        &root,
        "code/everything.sql",
        b"CREATE VIEW orders_v AS SELECT id FROM orders;\nCREATE PACKAGE secret_pkg AS END;",
    );
    let message = denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/everything.sql")),
    );
    assert!(message.contains("hidden"), "{message}");
    assert!(!message.contains("secret_pkg"), "{message}");
}

#[test]
fn hidden_names_match_whatever_their_object_type() {
    let (h, root) = compare_object_harness(SERVER_VIEW_DDL);
    h.set_policy(McpPolicy {
        allow_compare_object: true,
        object_deny: vec![McpObjectDenyRule {
            object_type: "INDEX".into(),
            name: "SECRET_IX".into(),
        }],
        ..enabled_policy(1)
    });
    write_schema_file(&root, "code/a.sql", b"CREATE INDEX secret_ix ON t(a);");
    let message = denied_message(
        h.gateway
            .compare_object(CLIENT, compare_object_args("code/a.sql")),
    );
    assert!(message.contains("hidden"), "{message}");
}
