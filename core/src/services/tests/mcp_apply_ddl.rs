//! `apply_ddl` through the gateway: policy gating, the approval handshake, DDL validation
//! and the audit trail, against the in-memory fakes.

use std::sync::atomic::Ordering;

use serde_json::Value;

use super::*;
use crate::models::{DbType, McpObjectDenyRule, McpPolicy};
use crate::repositories::db_repository::DdlCompileError;
use crate::repositories::mcp_policy_repository::{McpPolicyRepository, SqliteMcpPolicyRepository};
use crate::services::mcp_apply_approval::ApplyStatus;
use crate::test_support::{Harness, connection, enabled_policy};

const CLIENT: &str = "test-agent 1.0";
const VIEW_DDL: &str = "CREATE OR REPLACE VIEW v_open AS SELECT 1 AS one FROM dual";

fn deploy_policy(requires_approval: bool) -> McpPolicy {
    McpPolicy {
        allow_apply_ddl: true,
        apply_ddl_requires_approval: requires_approval,
        ..enabled_policy(1)
    }
}

fn harness(policy: McpPolicy) -> Harness {
    let h = Harness::new(vec![connection(1, "Dev", DbType::Oracle)]);
    h.set_policy(policy);
    h
}

fn args(object_name: &str, object_type: &str, ddl: &str) -> ApplyDdlArgs {
    ApplyDdlArgs {
        connection_id: 1,
        object_name: object_name.into(),
        object_type: object_type.into(),
        ddl: Some(ddl.into()),
        file: None,
    }
}

fn view_args() -> ApplyDdlArgs {
    args("V_OPEN", "VIEW", VIEW_DDL)
}

fn denied_message(result: ToolResult) -> String {
    match result {
        Err(ToolError::Denied(message)) => message,
        other => panic!("expected a denial, got {other:?}"),
    }
}

fn last_audit(h: &Harness) -> (crate::models::McpAuditEntry, Value) {
    let entry = h.entries().pop().expect("an audit entry");
    let arguments = serde_json::from_str(&entry.arguments).unwrap();
    (entry, arguments)
}

fn approve(h: &Harness, pending: &Value) {
    assert!(
        h.approvals
            .decide(pending["approval_id"].as_i64().unwrap(), true)
            .unwrap()
    );
}

// ── policy defaults and migration ───────────────────────────────────────────────

#[test]
fn deploys_are_off_and_need_approval_by_default() {
    let policy = McpPolicy::default();
    assert!(!policy.allow_apply_ddl);
    assert!(policy.apply_ddl_requires_approval);
    assert!(policy.check_tool(McpTool::ApplyDdl).is_err());
    assert_eq!(McpTool::ApplyDdl.name(), "apply_ddl");
    assert!(McpTool::ALL.contains(&McpTool::ApplyDdl));

    let enabled = enabled_policy(1);
    assert!(
        enabled
            .check_tool(McpTool::ApplyDdl)
            .unwrap_err()
            .contains("allow_apply_ddl")
    );
    assert_eq!(enabled.agent_view()["apply_ddl"], false);
    assert!(deploy_policy(true).check_tool(McpTool::ApplyDdl).is_ok());
    assert_eq!(
        deploy_policy(true).agent_view()["apply_ddl_requires_approval"],
        true
    );
}

#[test]
fn policies_stored_before_the_capability_existed_keep_deploys_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let old = rusqlite::Connection::open(&path).unwrap();
    old.execute_batch(
        "CREATE TABLE connection_mcp_policy (
            connection_id INTEGER PRIMARY KEY, mcp_enabled INTEGER NOT NULL DEFAULT 0,
            allow_schema_list INTEGER NOT NULL DEFAULT 1, allow_ddl_read INTEGER NOT NULL DEFAULT 1,
            allow_save_scripts INTEGER NOT NULL DEFAULT 0, allow_compare_fix INTEGER NOT NULL DEFAULT 0,
            data_access TEXT NOT NULL DEFAULT 'none', allow_raw_sql INTEGER NOT NULL DEFAULT 0,
            table_rules TEXT NOT NULL DEFAULT '[]', object_deny TEXT NOT NULL DEFAULT '[]',
            max_rows INTEGER NOT NULL DEFAULT 200, query_timeout_secs INTEGER NOT NULL DEFAULT 30,
            mcp_username TEXT NOT NULL DEFAULT '');
         INSERT INTO connection_mcp_policy (connection_id, mcp_enabled) VALUES (7, 1);",
    )
    .unwrap();
    drop(old);

    let repo = SqliteMcpPolicyRepository::with_path(path);
    repo.init_db().unwrap();
    repo.init_db().unwrap(); // idempotent
    let migrated = repo.get(7).unwrap().unwrap();
    assert!(migrated.mcp_enabled);
    assert!(!migrated.allow_apply_ddl);
    assert!(migrated.apply_ddl_requires_approval);

    repo.upsert(&McpPolicy {
        allow_apply_ddl: true,
        apply_ddl_requires_approval: false,
        ..migrated
    })
    .unwrap();
    let saved = repo.get(7).unwrap().unwrap();
    assert!(saved.allow_apply_ddl && !saved.apply_ddl_requires_approval);
}

// ── gating ──────────────────────────────────────────────────────────────────────

#[test]
fn a_connection_without_the_flag_is_denied_before_anything_else() {
    let h = harness(enabled_policy(1));
    let message = denied_message(h.gateway.apply_ddl(CLIENT, view_args()));
    assert!(message.contains("allow_apply_ddl"));
    assert_eq!(h.db_calls(), 0);
    assert!(h.approvals.list_open().unwrap().is_empty());
    assert_eq!(h.entries().len(), 1);
    assert_eq!(h.entries()[0].outcome, McpOutcome::Denied);
    assert_eq!(h.entries()[0].tool, "apply_ddl");
}

#[test]
fn unknown_and_disabled_connections_are_denied_like_for_every_tool() {
    let h = harness(McpPolicy {
        mcp_enabled: false,
        ..deploy_policy(false)
    });
    let disabled = denied_message(h.gateway.apply_ddl(CLIENT, view_args()));
    let mut other = view_args();
    other.connection_id = 99;
    let unknown = denied_message(h.gateway.apply_ddl(CLIENT, other));
    assert_eq!(disabled.replace('1', "N"), unknown.replace("99", "N"));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn hidden_objects_cannot_be_deployed() {
    let h = harness(McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "VIEW".into(),
            name: "V_*".into(),
        }],
        ..deploy_policy(false)
    });
    assert!(denied_message(h.gateway.apply_ddl(CLIENT, view_args())).contains("hidden by policy"));
    assert_eq!(h.db_calls(), 0);
}

// ── approval handshake ──────────────────────────────────────────────────────────

#[test]
fn the_first_call_only_files_a_request_and_changes_nothing() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();

    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["executed"], false);
    assert_eq!(pending["plan"]["statement"], "CREATE VIEW");
    assert_eq!(pending["plan"]["object_name"], "V_OPEN");
    assert_eq!(pending["plan"]["bytes"], VIEW_DDL.len());
    assert_eq!(pending["plan"]["sha256"].as_str().unwrap().len(), 64);
    assert!(pending["message"].as_str().unwrap().contains("approve"));
    assert!(h.db.applied.lock().unwrap().is_empty());
    assert_eq!(h.db_calls(), 0);

    let open = h.approvals.list_open().unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].ddl, VIEW_DDL);
    assert_eq!(open[0].status, ApplyStatus::Pending);

    let (entry, arguments) = last_audit(&h);
    assert_eq!(h.entries().len(), 1);
    assert_eq!(entry.outcome, McpOutcome::Allowed);
    assert_eq!(arguments["status"], "pending");
    assert_eq!(arguments["approval_id"], open[0].id);
    assert_eq!(arguments["sha256"], pending["plan"]["sha256"]);
}

#[test]
fn asking_again_before_approval_reuses_the_same_request() {
    let h = harness(deploy_policy(true));
    let first = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    let second = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    assert_eq!(first["approval_id"], second["approval_id"]);
    assert_eq!(h.approvals.list_open().unwrap().len(), 1);
    assert!(h.db.applied.lock().unwrap().is_empty());
}

#[test]
fn an_approved_deploy_runs_once_on_the_connections_own_account() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &pending);

    let applied = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(applied["executed"], true);
    assert_eq!(applied["approval_id"], pending["approval_id"]);
    assert_eq!(applied["compiled_without_errors"], true);

    {
        let runs = h.db.applied.lock().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, "app");
        assert_eq!(runs[0].1, "V_OPEN");
        assert_eq!(runs[0].2[0].sql, VIEW_DDL);
    }

    let (entry, arguments) = last_audit(&h);
    assert_eq!(entry.outcome, McpOutcome::Allowed);
    assert_eq!(arguments["status"], "applied");
    assert_eq!(arguments["approval_id"], pending["approval_id"]);
    assert_eq!(arguments["ddl_bytes"], VIEW_DDL.len());

    // Single use: the next identical call needs a fresh approval.
    let again = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    assert_eq!(again["status"], "pending");
    assert_ne!(again["approval_id"], pending["approval_id"]);
    assert_eq!(h.db.applied.lock().unwrap().len(), 1);
}

#[test]
fn an_approval_only_covers_the_exact_ddl_and_object_that_was_reviewed() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &pending);

    // Same object, edited statement.
    let edited = args(
        "V_OPEN",
        "VIEW",
        "CREATE OR REPLACE VIEW v_open AS SELECT 2 AS one FROM dual",
    );
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, edited).unwrap()["status"],
        "pending"
    );
    // Same text under another object name.
    let other = args("V_OTHER", "VIEW", &VIEW_DDL.replace("v_open", "v_other"));
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, other).unwrap()["status"],
        "pending"
    );
    assert!(h.db.applied.lock().unwrap().is_empty());

    // The reviewed one still works, and line endings don't change its identity.
    let crlf = args("V_OPEN", "VIEW", &format!("{VIEW_DDL}\r\n"));
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, crlf).unwrap()["status"],
        "applied"
    );
}

#[test]
fn approvals_are_bound_to_their_connection() {
    let h = Harness::new(vec![
        connection(1, "Dev", DbType::Oracle),
        connection(2, "Test", DbType::Oracle),
    ]);
    h.set_policy(deploy_policy(true));
    h.set_policy(McpPolicy {
        connection_id: 2,
        ..deploy_policy(true)
    });
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &pending);

    let mut other = view_args();
    other.connection_id = 2;
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, other).unwrap()["status"],
        "pending"
    );
    assert!(h.db.applied.lock().unwrap().is_empty());
}

#[test]
fn unapproved_rejected_and_expired_requests_never_execute() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    let id = pending["approval_id"].as_i64().unwrap();

    // Rejected.
    assert!(h.approvals.decide(id, false).unwrap());
    assert!(
        !h.approvals.decide(id, true).unwrap(),
        "a decision is final"
    );
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, view_args()).unwrap()["status"],
        "pending"
    );

    // Approved but not used in time.
    let second = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &second);
    h.clock.fetch_add(16 * 60, Ordering::SeqCst);
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, view_args()).unwrap()["status"],
        "pending"
    );

    // Pending for too long: it can no longer be approved.
    let stale = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    h.clock.fetch_add(61 * 60, Ordering::SeqCst);
    assert!(
        !h.approvals
            .decide(stale["approval_id"].as_i64().unwrap(), true)
            .unwrap()
    );
    assert!(h.db.applied.lock().unwrap().is_empty());
}

#[test]
fn switching_the_flag_off_revokes_outstanding_approvals() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &pending);
    h.set_policy(enabled_policy(1));
    assert!(matches!(
        h.gateway.apply_ddl(CLIENT, view_args()),
        Err(ToolError::Denied(_))
    ));
    assert!(h.db.applied.lock().unwrap().is_empty());
}

#[test]
fn without_the_approval_requirement_a_valid_deploy_runs_immediately() {
    let h = harness(deploy_policy(false));
    let applied = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    assert_eq!(applied["status"], "applied");
    assert!(applied["approval_id"].is_null());
    assert_eq!(h.db.applied.lock().unwrap().len(), 1);
    assert!(h.approvals.list_open().unwrap().is_empty());
}

// ── validation ──────────────────────────────────────────────────────────────────

#[test]
fn invalid_ddl_is_denied_audited_and_never_requests_approval() {
    let h = harness(deploy_policy(true));
    for (ddl, ty) in [
        ("DROP TABLE orders", "VIEW"),
        ("TRUNCATE TABLE orders", "VIEW"),
        ("GRANT DBA TO app", "VIEW"),
        (
            "CREATE OR REPLACE VIEW v_open AS SELECT 1 FROM dual; DROP TABLE orders;",
            "VIEW",
        ),
        (
            "CREATE OR REPLACE VIEW v_other AS SELECT 1 FROM dual",
            "VIEW",
        ),
        (
            "CREATE OR REPLACE VIEW hr.v_open AS SELECT 1 FROM dual",
            "VIEW",
        ),
        (VIEW_DDL, "PROCEDURE"),
        (VIEW_DDL, "TABLE"),
    ] {
        let result = h.gateway.apply_ddl(CLIENT, args("V_OPEN", ty, ddl));
        assert!(matches!(result, Err(ToolError::Denied(_))), "{ddl} / {ty}");
    }
    assert!(
        denied_message(
            h.gateway
                .apply_ddl(CLIENT, args("v open", "VIEW", VIEW_DDL))
        )
        .contains("identifier")
    );
    assert!(
        denied_message(h.gateway.apply_ddl(CLIENT, args("V_OPEN", "VIEW", "  ")))
            .contains("non-empty")
    );

    assert_eq!(h.db_calls(), 0);
    assert!(h.approvals.list_open().unwrap().is_empty());
    assert!(h.entries().iter().all(|e| e.outcome == McpOutcome::Denied));
    assert_eq!(h.entries().len(), 10);
}

#[test]
fn exactly_one_of_ddl_or_file_is_required() {
    let h = harness(deploy_policy(false));
    let mut neither = view_args();
    neither.ddl = None;
    assert!(denied_message(h.gateway.apply_ddl(CLIENT, neither)).contains("exactly one"));
    let mut both = view_args();
    both.file = Some("code/v.sql".into());
    assert!(denied_message(h.gateway.apply_ddl(CLIENT, both)).contains("exactly one"));
    assert_eq!(h.db_calls(), 0);
}

#[test]
fn postgres_deploys_validate_against_the_connections_schema() {
    let mut conn = connection(1, "PgDev", DbType::Postgres);
    conn.pg_schema = "sales".into();
    let h = Harness::new(vec![conn]);
    h.set_policy(deploy_policy(false));

    let function =
        "CREATE OR REPLACE FUNCTION sales.f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql";
    let applied = h
        .gateway
        .apply_ddl(CLIENT, args("f", "function", function))
        .unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(h.db.applied.lock().unwrap()[0].1, "f");

    let foreign = function.replace("sales.f", "public.f");
    assert!(
        denied_message(h.gateway.apply_ddl(CLIENT, args("f", "FUNCTION", &foreign)))
            .contains("own schema")
    );
}

// ── execution results ───────────────────────────────────────────────────────────

#[test]
fn compile_errors_are_reported_alongside_success() {
    let h = harness(deploy_policy(false));
    *h.db.compile_errors.lock().unwrap() = vec![DdlCompileError {
        object_type: "PROCEDURE".into(),
        severity: "ERROR".into(),
        line: 3,
        position: 5,
        text: "PLS-00201: identifier 'X' must be declared".into(),
    }];
    let ddl = "CREATE OR REPLACE PROCEDURE p IS BEGIN x; END;\n/";
    let result = h
        .gateway
        .apply_ddl(CLIENT, args("P", "PROCEDURE", ddl))
        .unwrap();
    assert_eq!(result["status"], "applied");
    assert_eq!(result["compiled_without_errors"], false);
    assert_eq!(result["compile_errors"][0]["line"], 3);
    assert!(
        result["compile_errors"][0]["text"]
            .as_str()
            .unwrap()
            .contains("PLS-00201")
    );
    let (_, arguments) = last_audit(&h);
    assert_eq!(arguments["compile_errors"], 1);
}

#[test]
fn a_failing_deploy_is_an_audited_error_and_spends_the_approval() {
    let h = harness(deploy_policy(true));
    let pending = h.gateway.apply_ddl(CLIENT, view_args()).unwrap();
    approve(&h, &pending);
    *h.db.apply_failure.lock().unwrap() = Some("ORA-00942: table or view does not exist".into());

    let result = h.gateway.apply_ddl(CLIENT, view_args());
    assert!(matches!(&result, Err(ToolError::Failed(m)) if m.contains("ORA-00942")));
    let (entry, arguments) = last_audit(&h);
    assert_eq!(entry.outcome, McpOutcome::Error);
    assert_eq!(arguments["approval_id"], pending["approval_id"]);

    *h.db.apply_failure.lock().unwrap() = None;
    assert_eq!(
        h.gateway.apply_ddl(CLIENT, view_args()).unwrap()["status"],
        "pending"
    );
}

#[test]
fn ddl_content_never_reaches_the_audit_log() {
    let h = harness(deploy_policy(true));
    let secret = "CREATE OR REPLACE VIEW v_open AS SELECT 'hunter2-confidential' AS s FROM dual";
    let call = || args("V_OPEN", "VIEW", secret);
    let pending = h.gateway.apply_ddl(CLIENT, call()).unwrap();
    approve(&h, &pending);
    h.gateway.apply_ddl(CLIENT, call()).unwrap();
    let denied = args("V_OPEN", "VIEW", "DROP TABLE hunter2-confidential");
    let _ = h.gateway.apply_ddl(CLIENT, denied);

    assert_eq!(h.entries().len(), 3);
    for entry in h.entries() {
        assert!(!format!("{entry:?}").contains("hunter2"));
    }
}

#[test]
fn the_result_is_withheld_when_the_audit_entry_cannot_be_written() {
    let h = harness(deploy_policy(false));
    h.audit.fail_appends.store(true, Ordering::SeqCst);
    let result = h.gateway.apply_ddl(CLIENT, view_args());
    assert!(matches!(&result, Err(ToolError::Failed(m)) if m.contains("audit log")));
}

// ── file source ─────────────────────────────────────────────────────────────────

fn file_args(path: &str) -> ApplyDdlArgs {
    ApplyDdlArgs {
        ddl: None,
        file: Some(path.into()),
        ..view_args()
    }
}

#[test]
fn a_ddl_file_inside_the_schema_folder_can_be_deployed() {
    let root = tempfile::tempdir().unwrap();
    let code = root.path().join("app").join("code");
    std::fs::create_dir_all(&code).unwrap();
    std::fs::write(code.join("v_open.sql"), format!("{VIEW_DDL};\r\n")).unwrap();
    std::fs::write(root.path().join("secret.sql"), VIEW_DDL).unwrap();

    let h = harness(deploy_policy(false));
    *h.settings.schema_root.lock().unwrap() = Some(root.path().to_string_lossy().into_owned());

    let applied = h
        .gateway
        .apply_ddl(CLIENT, file_args("code/v_open.sql"))
        .unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(h.db.applied.lock().unwrap()[0].2[0].sql, VIEW_DDL);
    let (_, arguments) = last_audit(&h);
    assert_eq!(arguments["file"], "code/v_open.sql");

    for escape in [
        "../secret.sql",
        "code/../../secret.sql",
        "/etc/passwd",
        "C:\\Windows\\win.ini",
        "",
    ] {
        let message = denied_message(h.gateway.apply_ddl(CLIENT, file_args(escape)));
        assert!(message.contains("relative path"), "{escape}: {message}");
    }
    assert!(matches!(
        h.gateway.apply_ddl(CLIENT, file_args("code/missing.sql")),
        Err(ToolError::Failed(_))
    ));
    assert_eq!(h.db.applied.lock().unwrap().len(), 1);
}
