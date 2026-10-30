//! MCP gateway against two real Postgres instances (the docker-compose `postgres-*`
//! containers): list → read DDL → save to a temp schema root, with policy enforcement and
//! the audit trail checked end to end. Policies and the audit log live in a throwaway
//! SQLite file; connections and settings are in-memory so no keychain is touched.
//!
//!   cd docker && docker compose up -d --build
//!   cd .. && cargo test --test mcp_integration -- --ignored
//!
//! Unit-level policy/audit/guard coverage (which needs no database) lives in
//! `core/src/services/tests/mcp_*.rs`.

mod common;

use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use schemetry_core::models::*;
use schemetry_core::repositories::connection_repository::ConnectionRepository;
use schemetry_core::repositories::db_repository::{DbRepository, ReadOnlyQuery};
use schemetry_core::repositories::dispatch_repository::DispatchRepository;
use schemetry_core::repositories::filter_rule_repository::FilterRuleRepository;
use schemetry_core::repositories::folder_schema_override_repository::SchemaFolderOverrideRepository;
use schemetry_core::repositories::mcp_audit_repository::SqliteMcpAuditRepository;
use schemetry_core::repositories::mcp_policy_repository::SqliteMcpPolicyRepository;
use schemetry_core::services::connection_catalog::ConnectionCatalogService;
use schemetry_core::services::ddl_save::DdlSaveOptions;
use schemetry_core::services::filter_rules::FilterRuleService;
use schemetry_core::services::folder_schema_overrides::SchemaFolderOverrideService;
use schemetry_core::services::mcp_audit::McpAuditService;
use schemetry_core::services::mcp_gateway::*;
use schemetry_core::services::mcp_policy::{AccountPassword, McpPolicyService};
use schemetry_core::services::query::QueryService;
use schemetry_core::services::schema_diff::SchemaDiffService;
use serde_json::json;

use common::postgres as pg;

const CLIENT: &str = "integration-test 1.0";

struct Connections(Vec<ConnectionRecord>);

impl ConnectionRepository for Connections {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn get_all_connections(&self) -> Result<Vec<ConnectionRecord>> {
        Ok(self.0.clone())
    }
    fn list_connection_summaries(&self) -> Result<Vec<ConnectionRecord>> {
        Ok(self
            .0
            .iter()
            .cloned()
            .map(|mut c| {
                c.password.clear();
                c
            })
            .collect())
    }
    fn get_connection(&self, id: i64) -> Result<Option<ConnectionRecord>> {
        Ok(self.0.iter().find(|c| c.id == id).cloned())
    }
    fn insert_connection(&self, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("unsupported"))
    }
    fn update_connection(&self, _: i64, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("unsupported"))
    }
    fn delete_connection(&self, _: i64) -> Result<()> {
        Err(anyhow!("unsupported"))
    }
    fn delete_all_connections(&self) -> Result<()> {
        Err(anyhow!("unsupported"))
    }
    fn upsert_connection(&self, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("unsupported"))
    }
}

struct NoRules;

impl FilterRuleRepository for NoRules {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn list_rules(&self) -> Result<Vec<TableFilterRule>> {
        Ok(Vec::new())
    }
    fn insert_rule(&self, r: &TableFilterRule) -> Result<TableFilterRule> {
        Ok(r.clone())
    }
    fn update_rule(&self, _: i64, r: &TableFilterRule) -> Result<TableFilterRule> {
        Ok(r.clone())
    }
    fn delete_rule(&self, _: i64) -> Result<()> {
        Ok(())
    }
}

struct NoOverrides;

impl SchemaFolderOverrideRepository for NoOverrides {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>> {
        Ok(Vec::new())
    }
    fn insert_override(&self, d: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        Ok(d.clone())
    }
    fn update_override(&self, _: i64, d: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        Ok(d.clone())
    }
    fn delete_override(&self, _: i64) -> Result<()> {
        Ok(())
    }
    fn find_for_schema(&self, _: &str) -> Result<Option<SchemaFolderOverride>> {
        Ok(None)
    }
}

struct Settings {
    root: Mutex<Option<String>>,
}

impl GatewaySettings for Settings {
    fn data_tools_disabled(&self) -> bool {
        false
    }
    fn schema_root(&self) -> Result<String, String> {
        self.root
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "no schema root".to_string())
    }
    fn save_options(&self, _: &str) -> DdlSaveOptions {
        DdlSaveOptions {
            code_folder: "code".into(),
            migration_folder: "migration".into(),
            storage_mode: "both".into(),
            naming_convention: "flyway".into(),
            folder_mode: "year".into(),
            version_label: String::new(),
            extension: "sql".into(),
            encoding: "utf8".into(),
        }
    }
}

struct Env {
    gateway: McpGateway,
    policies: McpPolicyService,
    audit: McpAuditService,
    repo: Arc<dyn DbRepository>,
    schema_root: tempfile::TempDir,
    _db_dir: tempfile::TempDir,
}

fn env() -> Env {
    env_with(pg::target_connection())
}

fn env_with(second: ConnectionRecord) -> Env {
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("mcp.db");
    let schema_root = tempfile::tempdir().unwrap();

    let policy_repo = || Arc::new(SqliteMcpPolicyRepository::with_path(db_path.clone()));
    let audit_repo = || Arc::new(SqliteMcpAuditRepository::with_path(db_path.clone()));
    let policies = McpPolicyService::new(policy_repo());
    policies.init_db().unwrap();
    let audit = McpAuditService::new(audit_repo()).without_forwarding();
    audit.init_db().unwrap();

    let repo: Arc<dyn DbRepository> = Arc::new(DispatchRepository::new());
    let settings = Arc::new(Settings {
        root: Mutex::new(Some(schema_root.path().to_string_lossy().into_owned())),
    });
    let gateway = McpGateway::from_parts(
        Arc::new(ConnectionCatalogService::new(Arc::new(Connections(vec![
            pg::source_connection(),
            second,
        ])))),
        Arc::new(SchemaDiffService::new(Arc::clone(&repo))),
        Arc::new(QueryService::new(Arc::clone(&repo))),
        Arc::new(FilterRuleService::new(Arc::new(NoRules))),
        Arc::new(SchemaFolderOverrideService::new(Arc::new(NoOverrides))),
        Arc::new(McpPolicyService::new(policy_repo())),
        Arc::new(McpAuditService::new(audit_repo()).without_forwarding()),
        settings,
    );
    Env {
        gateway,
        policies,
        audit,
        repo,
        schema_root,
        _db_dir: db_dir,
    }
}

fn rule(action: McpRuleAction, table: &str, column: &str) -> McpTableRule {
    McpTableRule {
        action,
        table: table.into(),
        column: column.into(),
    }
}

fn set(env: &Env, policy: McpPolicy) {
    env.policies.save(policy, AccountPassword::Keep).unwrap();
}

fn base(id: i64) -> McpPolicy {
    McpPolicy {
        connection_id: id,
        mcp_enabled: true,
        ..McpPolicy::default()
    }
}

fn query(table: &str) -> QueryTableArgs {
    QueryTableArgs {
        connection_id: pg::source_connection().id,
        table: table.into(),
        columns: None,
        filters: None,
        order_by: None,
        limit: None,
    }
}

const ID: fn() -> i64 = || pg::source_connection().id;

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn list_read_ddl_and_save_scripts_end_to_end() {
    let env = env();
    set(
        &env,
        McpPolicy {
            allow_save_scripts: true,
            ..base(ID())
        },
    );

    let listed = env
        .gateway
        .list_schema_objects(
            CLIENT,
            ListSchemaObjectsArgs {
                connection_id: ID(),
                object_type: Some("TABLE".into()),
            },
        )
        .unwrap();
    let names: Vec<&str> = listed["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap())
        .collect();
    for expected in ["departments", "employees", "audit_log"] {
        assert!(names.contains(&expected), "{names:?}");
    }

    let ddl = env
        .gateway
        .get_object_ddl(
            CLIENT,
            GetObjectDdlArgs {
                connection_id: ID(),
                object_name: "employees".into(),
                object_type: "TABLE".into(),
            },
        )
        .unwrap();
    let ddl_text = ddl["ddl"].as_str().unwrap().to_string();
    assert!(ddl_text.to_lowercase().contains("create table"));

    let save = |dry_run: bool| SaveMigrationScriptArgs {
        connection_id: ID(),
        object_name: "employees".into(),
        object_type: "TABLE".into(),
        ddl: ddl_text.clone(),
        description: "from_mcp".into(),
        dry_run,
    };
    let preview = env
        .gateway
        .save_migration_script(CLIENT, save(true))
        .unwrap();
    assert_eq!(preview["written"], false);
    assert_eq!(
        std::fs::read_dir(env.schema_root.path()).unwrap().count(),
        0
    );

    let saved = env
        .gateway
        .save_migration_script(CLIENT, save(false))
        .unwrap();
    let files = saved["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for file in files {
        let path = std::path::PathBuf::from(file["path"].as_str().unwrap());
        assert!(path.starts_with(env.schema_root.path()));
        assert!(path.is_file(), "{path:?}");
    }
    let migration = files.iter().find(|f| f["kind"] == "migration").unwrap();
    assert!(
        migration["path"]
            .as_str()
            .unwrap()
            .contains("V1__public_from_mcp.sql")
    );

    // One entry per call, chain intact.
    let entries = env.audit.list(&McpAuditFilter::default()).unwrap();
    assert_eq!(entries.len(), 4);
    assert!(entries.iter().all(|e| e.outcome == McpOutcome::Allowed));
    assert!(env.audit.verify().unwrap().ok);
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn nothing_is_reachable_until_the_connection_is_enabled() {
    let env = env();
    let denied = env.gateway.list_schema_objects(
        CLIENT,
        ListSchemaObjectsArgs {
            connection_id: ID(),
            object_type: None,
        },
    );
    assert!(matches!(denied, Err(ToolError::Denied(_))));
    let listed = env.gateway.list_connections(CLIENT).unwrap();
    assert!(listed["connections"].as_array().unwrap().is_empty());

    let entries = env.audit.list(&McpAuditFilter::default()).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(env.audit.verify().unwrap().ok);
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn default_policy_denies_table_data_and_writes_a_denied_entry() {
    let env = env();
    set(&env, base(ID()));

    let result = env.gateway.query_table(CLIENT, query("employees"));
    assert!(matches!(result, Err(ToolError::Denied(_))), "{result:?}");

    let entries = env.audit.list(&McpAuditFilter::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].outcome, McpOutcome::Denied);
    assert_eq!(entries[0].tool, "query_table");
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn table_and_column_rules_are_enforced_against_the_real_database() {
    let env = env();
    set(
        &env,
        McpPolicy {
            data_access: McpDataAccess::Allowlist,
            table_rules: vec![
                rule(McpRuleAction::Allow, "employees", ""),
                rule(McpRuleAction::Allow, "departments", ""),
                rule(McpRuleAction::Deny, "*", "email"),
                rule(McpRuleAction::Deny, "*", "salary"),
            ],
            max_rows: 100,
            ..base(ID())
        },
    );

    // Default columns exclude the denied ones.
    let out = env.gateway.query_table(CLIENT, query("employees")).unwrap();
    let columns: Vec<&str> = out["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert_eq!(columns, ["emp_id", "first_name", "last_name", "dept_id"]);
    assert_eq!(out["row_count"], 2);
    assert_eq!(out["omitted_columns"], 2);
    let text = out.to_string();
    assert!(text.contains("Ada") && !text.contains("schemetry.test"));

    // Asking for a denied column, or filtering/ordering by one, is refused.
    let mut args = query("employees");
    args.columns = Some(vec!["email".into()]);
    assert!(matches!(
        env.gateway.query_table(CLIENT, args),
        Err(ToolError::Denied(_))
    ));
    let mut args = query("employees");
    args.filters = Some(vec![schemetry_core::services::mcp_query_builder::Filter {
        column: "salary".into(),
        op: schemetry_core::services::mcp_query_builder::FilterOp::Gt,
        value: Some(json!(9000)),
    }]);
    assert!(matches!(
        env.gateway.query_table(CLIENT, args),
        Err(ToolError::Denied(_))
    ));

    // A table outside the allowlist is refused, an allowed one filters correctly.
    assert!(matches!(
        env.gateway.query_table(CLIENT, query("audit_log")),
        Err(ToolError::Denied(_))
    ));
    let mut args = query("employees");
    args.columns = Some(vec!["first_name".into()]);
    args.filters = Some(vec![schemetry_core::services::mcp_query_builder::Filter {
        column: "last_name".into(),
        op: schemetry_core::services::mcp_query_builder::FilterOp::Eq,
        value: Some(json!("Hopper")),
    }]);
    let out = env.gateway.query_table(CLIENT, args).unwrap();
    assert_eq!(out["rows"], json!([["Grace"]]));

    // Quotes in values are inert.
    let mut args = query("employees");
    args.columns = Some(vec!["first_name".into()]);
    args.filters = Some(vec![schemetry_core::services::mcp_query_builder::Filter {
        column: "last_name".into(),
        op: schemetry_core::services::mcp_query_builder::FilterOp::Eq,
        value: Some(json!("x'; DROP TABLE employees; --")),
    }]);
    let out = env.gateway.query_table(CLIENT, args).unwrap();
    assert_eq!(out["row_count"], 0);
    assert_eq!(
        env.gateway.query_table(CLIENT, query("employees")).unwrap()["row_count"],
        2
    );
    assert!(env.audit.verify().unwrap().ok);
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn the_row_limit_is_applied_by_the_database_layer() {
    let env = env();
    set(
        &env,
        McpPolicy {
            data_access: McpDataAccess::Allowlist,
            table_rules: vec![rule(McpRuleAction::Allow, "employees", "")],
            max_rows: 1,
            ..base(ID())
        },
    );
    let out = env.gateway.query_table(CLIENT, query("employees")).unwrap();
    assert_eq!(out["row_count"], 1);
    assert_eq!(out["truncated"], true);

    let mut args = query("employees");
    args.limit = Some(500);
    let out = env.gateway.query_table(CLIENT, args).unwrap();
    assert_eq!(out["row_count"], 1);
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn raw_sql_is_checked_and_runs_read_only() {
    let env = env();
    set(
        &env,
        McpPolicy {
            data_access: McpDataAccess::Allowlist,
            allow_raw_sql: true,
            table_rules: vec![
                rule(McpRuleAction::Allow, "employees", ""),
                rule(McpRuleAction::Allow, "departments", ""),
            ],
            ..base(ID())
        },
    );
    let run = |sql: &str| {
        env.gateway.run_query(
            CLIENT,
            RunQueryArgs {
                connection_id: ID(),
                sql: sql.into(),
                max_rows: None,
            },
        )
    };

    let out = run(
        "SELECT d.dept_name, count(*) AS n FROM employees e JOIN departments d ON d.dept_id = e.dept_id GROUP BY d.dept_name;",
    )
    .unwrap();
    assert_eq!(out["rows"], json!([["Engineering", "2"]]));

    for sql in [
        "SELECT * FROM audit_log",
        "SELECT * FROM pg_user",
        "SELECT * FROM employees; DELETE FROM employees",
        "DELETE FROM employees",
        "WITH d AS (DELETE FROM employees RETURNING *) SELECT * FROM d",
        "SELECT pg_read_file('/etc/passwd')",
    ] {
        assert!(
            matches!(run(sql), Err(ToolError::Denied(_))),
            "should be denied: {sql}"
        );
    }
    assert_eq!(
        env.gateway.query_table(CLIENT, query("employees")).unwrap()["row_count"],
        2
    );
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn the_database_layer_itself_refuses_writes_and_enforces_the_timeout() {
    let env = env();
    let conn = pg::source_connection();
    let run = |sql: &str, timeout_secs: u32| {
        env.repo.run_readonly_query(
            &conn,
            &ReadOnlyQuery {
                sql: sql.into(),
                binds: vec![],
                max_rows: 10,
                timeout_secs,
            },
        )
    };

    // Even if the static checks were bypassed, Postgres rejects writes in the cursor/transaction.
    let write = run(
        "WITH d AS (DELETE FROM employees WHERE emp_id = -1 RETURNING *) SELECT * FROM d",
        5,
    );
    assert!(write.is_err());
    assert!(run("SELECT 1; DROP TABLE employees", 5).is_err());

    let slow = run("SELECT pg_sleep(5)", 1).err().expect("should time out");
    assert!(
        slow.to_string().to_lowercase().contains("timed out"),
        "{slow:#}"
    );

    let plain = run("SELECT count(*) FROM employees", 5).unwrap();
    assert_eq!(plain.rows[0][0].as_deref(), Some("2"));
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn compare_and_fix_scripts_work_across_connections_when_both_allow_it() {
    // The shared TARGET container may already have been brought in line by the other
    // suites' fix tests, so build a deliberately divergent schema next to `public`.
    let mut divergent = pg::source_connection();
    divergent.id = 2;
    divergent.name = "DIVERGENT".into();
    divergent.pg_schema = "mcp_it".into();
    pg::execute_script(
        &pg::source_connection(),
        "DROP SCHEMA IF EXISTS mcp_it CASCADE;\n\nCREATE SCHEMA mcp_it;\n\n\
         CREATE TABLE mcp_it.employees (emp_id integer NOT NULL, first_name varchar(10));",
    );
    let env = env_with(divergent);
    let (source, target) = (pg::source_connection().id, 2);
    set(
        &env,
        McpPolicy {
            allow_compare_fix: true,
            ..base(source)
        },
    );

    let args = || CompareSchemasArgs {
        source_id: source,
        target_ids: vec![target],
        check_tables: None,
        check_columns: None,
        check_comments: None,
        check_indexes: None,
    };
    // The target connection has no policy yet: the whole call is refused.
    assert!(matches!(
        env.gateway.compare_schemas(CLIENT, args()),
        Err(ToolError::Denied(_))
    ));

    set(
        &env,
        McpPolicy {
            allow_compare_fix: true,
            ..base(target)
        },
    );
    let out = env.gateway.compare_schemas(CLIENT, args()).unwrap();
    let text = out.to_string();
    assert!(text.contains("audit_log"), "{text}");
    assert!(text.contains("email"), "{text}");

    let fix = env
        .gateway
        .generate_fix_script(
            CLIENT,
            GenerateFixScriptArgs {
                source_id: source,
                target_ids: vec![target],
                tables: None,
                check_tables: None,
                check_columns: None,
                check_comments: None,
                check_indexes: None,
            },
        )
        .unwrap();
    let script = fix["script"].as_str().unwrap().to_lowercase();
    assert!(
        script.contains("audit_log") && script.contains("email"),
        "{script}"
    );

    // Hiding a table removes it from compare output and from the generated script.
    set(
        &env,
        McpPolicy {
            allow_compare_fix: true,
            object_deny: vec![McpObjectDenyRule {
                object_type: "TABLE".into(),
                name: "audit_*".into(),
            }],
            ..base(target)
        },
    );
    let out = env.gateway.compare_schemas(CLIENT, args()).unwrap();
    assert!(!out.to_string().contains("audit_log"));
    assert!(out["hidden_by_policy"].as_u64().unwrap() >= 1);

    assert!(env.audit.verify().unwrap().ok);

    pg::execute_script(
        &pg::source_connection(),
        "DROP SCHEMA IF EXISTS mcp_it CASCADE;",
    );
}

#[test]
#[ignore = "requires the Postgres containers from docker/docker-compose.yml"]
fn audit_entries_never_contain_credentials() {
    let env = env();
    set(&env, base(ID()));
    let _ = env.gateway.list_connections(CLIENT);
    let _ = env.gateway.list_schema_objects(
        CLIENT,
        ListSchemaObjectsArgs {
            connection_id: ID(),
            object_type: None,
        },
    );
    let password = pg::source_connection().password;
    for entry in env.audit.list(&McpAuditFilter::default()).unwrap() {
        assert!(!format!("{entry:?}").contains(&password));
    }
}
