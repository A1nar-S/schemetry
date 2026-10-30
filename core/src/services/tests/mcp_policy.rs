use std::sync::Arc;

use super::*;
use crate::models::{McpObjectDenyRule, McpPolicy};
use crate::test_support::{MemoryPolicies, allow, deny, enabled_policy};

fn policy_with(rules: Vec<McpTableRule>) -> McpPolicy {
    McpPolicy {
        data_access: McpDataAccess::Allowlist,
        table_rules: rules,
        ..enabled_policy(1)
    }
}

// ── wildcard matching ───────────────────────────────────────────────────────────

#[test]
fn wildcards_match_prefixes_suffixes_and_single_characters() {
    assert!(wildcard_match("REF_*", "REF_COUNTRY"));
    assert!(wildcard_match("*_HIST", "ORDERS_HIST"));
    assert!(wildcard_match("*PASSWORD*", "USER_PASSWORD_HASH"));
    assert!(wildcard_match("ORDER?", "ORDERS"));
    assert!(wildcard_match("*", "ANYTHING"));
    assert!(wildcard_match("A*B*C", "AxxBxxC"));
    assert!(!wildcard_match("REF_*", "XREF_COUNTRY"));
    assert!(!wildcard_match("ORDER?", "ORDER"));
    assert!(!wildcard_match("A*B", "ACDC"));
}

#[test]
fn wildcard_match_is_case_insensitive_unless_exact() {
    assert!(wildcard_match("orders", "ORDERS"));
    assert!(!wildcard_match_exact("orders", "ORDERS"));
    assert!(wildcard_match_exact("ord*", "orders"));
}

#[test]
fn backtracking_handles_repeated_stars() {
    assert!(wildcard_match("*A*A*A*", "XAXAXAX"));
    assert!(!wildcard_match("*A*A*A*", "XAXAX"));
}

// ── defaults ────────────────────────────────────────────────────────────────────

#[test]
fn default_policy_is_the_most_restrictive_useful_level() {
    let policy = McpPolicy::default();
    assert!(!policy.mcp_enabled);
    assert!(policy.allow_schema_list && policy.allow_ddl_read);
    assert!(!policy.allow_save_scripts && !policy.allow_compare_fix);
    assert_eq!(policy.data_access, McpDataAccess::None);
    assert!(!policy.allow_raw_sql);
    assert_eq!(policy.max_rows, 200);
    assert_eq!(policy.query_timeout_secs, 30);
}

#[test]
fn nothing_is_allowed_until_mcp_is_enabled() {
    let policy = McpPolicy::default();
    for tool in McpTool::ALL {
        assert!(policy.check_tool(tool).is_err(), "{tool:?}");
    }
}

#[test]
fn enabled_policy_allows_only_listing_and_ddl_by_default() {
    let policy = enabled_policy(1);
    assert!(policy.check_tool(McpTool::ListSchemaObjects).is_ok());
    assert!(policy.check_tool(McpTool::GetObjectDdl).is_ok());
    assert!(policy.check_tool(McpTool::SaveMigrationScript).is_err());
    assert!(policy.check_tool(McpTool::CompareSchemas).is_err());
    assert!(policy.check_tool(McpTool::GenerateFixScript).is_err());
    assert!(policy.check_tool(McpTool::QueryTable).is_err());
    assert!(policy.check_tool(McpTool::RunQuery).is_err());
}

#[test]
fn raw_sql_needs_both_the_flag_and_an_allowlist() {
    let mut policy = enabled_policy(1);
    policy.allow_raw_sql = true;
    assert!(policy.check_tool(McpTool::RunQuery).is_err());
    policy.data_access = McpDataAccess::Allowlist;
    assert!(policy.check_tool(McpTool::RunQuery).is_ok());
    policy.allow_raw_sql = false;
    assert!(policy.check_tool(McpTool::RunQuery).is_err());
    assert!(policy.check_tool(McpTool::QueryTable).is_ok());
}

#[test]
fn limits_are_clamped() {
    let policy = McpPolicy {
        max_rows: 5_000_000,
        query_timeout_secs: 0,
        mcp_username: "  reader ".to_string(),
        ..McpPolicy::default()
    }
    .normalized();
    assert_eq!(policy.max_rows, McpPolicy::MAX_ROWS_CEILING);
    assert_eq!(policy.query_timeout_secs, 1);
    assert_eq!(policy.mcp_username, "reader");
}

// ── table / column rules ────────────────────────────────────────────────────────

#[test]
fn no_rules_means_no_table_is_readable() {
    let policy = policy_with(vec![]);
    assert!(!policy.table_allowed("ORDERS", DbType::Oracle));
}

#[test]
fn data_access_none_blocks_even_with_allow_rules() {
    let policy = McpPolicy {
        data_access: McpDataAccess::None,
        table_rules: vec![allow("*", "")],
        ..enabled_policy(1)
    };
    assert!(!policy.table_allowed("ORDERS", DbType::Oracle));
    assert!(!policy.column_allowed("ORDERS", "ID", DbType::Oracle));
}

#[test]
fn allow_rules_open_matching_tables_only() {
    let policy = policy_with(vec![allow("ORDERS", ""), allow("REF_*", "")]);
    assert!(policy.table_allowed("ORDERS", DbType::Oracle));
    assert!(policy.table_allowed("ref_country", DbType::Oracle));
    assert!(!policy.table_allowed("CUSTOMERS", DbType::Oracle));
}

#[test]
fn deny_wins_over_allow_for_whole_tables() {
    let policy = policy_with(vec![allow("*", ""), deny("USERS", "")]);
    assert!(policy.table_allowed("ORDERS", DbType::Oracle));
    assert!(!policy.table_allowed("USERS", DbType::Oracle));
    assert!(!policy.table_allowed("users", DbType::Oracle));
}

#[test]
fn a_column_wildcard_of_star_denies_the_whole_table() {
    let policy = policy_with(vec![allow("*", ""), deny("USERS", "*")]);
    assert!(!policy.table_allowed("USERS", DbType::Oracle));
}

#[test]
fn column_deny_hides_that_column_everywhere() {
    let policy = policy_with(vec![allow("*", ""), deny("*", "*PASSWORD*")]);
    assert!(policy.column_allowed("USERS", "NAME", DbType::Oracle));
    assert!(!policy.column_allowed("USERS", "PASSWORD_HASH", DbType::Oracle));
    assert!(!policy.column_allowed("ACCOUNTS", "old_password", DbType::Oracle));
}

#[test]
fn column_deny_on_one_table_leaves_other_tables_alone() {
    let policy = policy_with(vec![allow("*", ""), deny("USERS", "EMAIL")]);
    assert!(!policy.column_allowed("USERS", "EMAIL", DbType::Oracle));
    assert!(policy.column_allowed("USERS", "NAME", DbType::Oracle));
    assert!(policy.column_allowed("CONTACTS", "EMAIL", DbType::Oracle));
}

#[test]
fn column_level_allow_limits_the_table_to_those_columns() {
    let policy = policy_with(vec![allow("ORDERS", "ID"), allow("ORDERS", "STATUS")]);
    assert!(policy.table_allowed("ORDERS", DbType::Oracle));
    assert!(policy.column_allowed("ORDERS", "ID", DbType::Oracle));
    assert!(policy.column_allowed("ORDERS", "status", DbType::Oracle));
    assert!(!policy.column_allowed("ORDERS", "TOTAL", DbType::Oracle));
}

#[test]
fn whole_table_allow_plus_column_allow_keeps_all_columns() {
    let policy = policy_with(vec![allow("ORDERS", ""), allow("ORDERS", "ID")]);
    assert!(policy.column_allowed("ORDERS", "TOTAL", DbType::Oracle));
}

#[test]
fn postgres_allow_rules_are_case_sensitive_but_deny_rules_are_not() {
    let policy = policy_with(vec![allow("orders", ""), allow("Mixed", "")]);
    assert!(policy.table_allowed("orders", DbType::Postgres));
    assert!(!policy.table_allowed("ORDERS", DbType::Postgres));
    assert!(policy.table_allowed("Mixed", DbType::Postgres));
    assert!(!policy.table_allowed("mixed", DbType::Postgres));
    // Oracle ignores case on allow rules.
    assert!(policy.table_allowed("ORDERS", DbType::Oracle));

    let denied = policy_with(vec![allow("*", ""), deny("secret", "")]);
    assert!(!denied.table_allowed("SECRET", DbType::Postgres));
    assert!(!denied.table_allowed("secret", DbType::Postgres));
}

#[test]
fn column_rules_are_detected_per_table() {
    let policy = policy_with(vec![
        allow("*", ""),
        deny("USERS", "EMAIL"),
        allow("ORDERS", "ID"),
    ]);
    assert!(policy.has_column_rules_for("USERS", DbType::Oracle));
    assert!(policy.has_column_rules_for("ORDERS", DbType::Oracle));
    assert!(!policy.has_column_rules_for("ITEMS", DbType::Oracle));
}

// ── object hiding ───────────────────────────────────────────────────────────────

#[test]
fn object_deny_rules_match_by_type_and_name() {
    let policy = McpPolicy {
        object_deny: vec![
            McpObjectDenyRule {
                object_type: "TABLE".into(),
                name: "AUDIT_*".into(),
            },
            McpObjectDenyRule {
                object_type: "*".into(),
                name: "SECRET_PKG".into(),
            },
        ],
        ..enabled_policy(1)
    };
    assert!(policy.object_denied("AUDIT_LOG", "TABLE"));
    assert!(policy.object_denied("audit_log", "table"));
    assert!(!policy.object_denied("AUDIT_LOG", "VIEW"));
    assert!(policy.object_denied("SECRET_PKG", "PACKAGE BODY"));
    assert!(!policy.object_denied("ORDERS", "TABLE"));
}

// ── validation ──────────────────────────────────────────────────────────────────

#[test]
fn validation_rejects_suspicious_patterns() {
    for bad in ["", "a b", "a;b", "a'b", "a\"b", "../x", "a.b", "a%"] {
        let policy = policy_with(vec![allow(bad, "")]);
        assert!(validate_policy(&policy).is_err(), "table pattern {bad:?}");
    }
    let bad_column = policy_with(vec![allow("T", "c;")]);
    assert!(validate_policy(&bad_column).is_err());
    let bad_object = McpPolicy {
        object_deny: vec![McpObjectDenyRule {
            object_type: "TABLE;".into(),
            name: "X".into(),
        }],
        ..McpPolicy::default()
    };
    assert!(validate_policy(&bad_object).is_err());
}

#[test]
fn validation_accepts_normal_patterns() {
    let policy = policy_with(vec![
        allow("ORDERS", ""),
        allow("REF_*", "ID"),
        deny("*", "*PASSWORD*"),
        deny("T$#1", "c-1"),
    ]);
    assert!(validate_policy(&policy).is_ok());
}

#[test]
fn agent_view_never_includes_the_account_name() {
    let policy = McpPolicy {
        mcp_username: "svc_mcp".into(),
        ..policy_with(vec![allow("ORDERS", "")])
    };
    let view = policy.agent_view().to_string();
    assert!(!view.contains("svc_mcp"));
    assert!(view.contains("ORDERS"));
}

// ── service ─────────────────────────────────────────────────────────────────────

fn service() -> (McpPolicyService, Arc<MemoryPolicies>) {
    let repo = Arc::new(MemoryPolicies::default());
    (McpPolicyService::new(repo.clone()), repo)
}

#[test]
fn connections_without_a_stored_policy_get_the_restrictive_default() {
    let (svc, _) = service();
    let policy = svc.get(42).unwrap();
    assert_eq!(policy.connection_id, 42);
    assert!(!policy.mcp_enabled);
    assert_eq!(policy.data_access, McpDataAccess::None);
}

#[test]
fn saving_validates_and_clamps() {
    let (svc, repo) = service();
    let bad = policy_with(vec![allow("a b", "")]);
    assert!(svc.save(bad, AccountPassword::Keep).is_err());
    assert!(repo.policies.lock().unwrap().is_empty());

    let saved = svc
        .save(
            McpPolicy {
                max_rows: u32::MAX,
                ..policy_with(vec![allow("ORDERS", "")])
            },
            AccountPassword::Keep,
        )
        .unwrap();
    assert_eq!(saved.max_rows, McpPolicy::MAX_ROWS_CEILING);
}

#[test]
fn account_password_lifecycle() {
    let (svc, repo) = service();
    let with_account = McpPolicy {
        mcp_username: "reader".into(),
        ..enabled_policy(7)
    };

    let saved = svc
        .save(with_account.clone(), AccountPassword::Set("pw".into()))
        .unwrap();
    assert!(saved.mcp_password_set);
    assert_eq!(svc.account_password(7).unwrap().as_deref(), Some("pw"));

    // Keeping leaves it alone.
    svc.save(with_account.clone(), AccountPassword::Keep)
        .unwrap();
    assert!(repo.passwords.lock().unwrap().contains_key(&7));

    // Clearing the username drops the now-useless password.
    svc.save(enabled_policy(7), AccountPassword::Keep).unwrap();
    assert!(!repo.passwords.lock().unwrap().contains_key(&7));

    svc.save(with_account, AccountPassword::Set("pw2".into()))
        .unwrap();
    svc.save(
        McpPolicy {
            mcp_username: "reader".into(),
            ..enabled_policy(7)
        },
        AccountPassword::Clear,
    )
    .unwrap();
    assert_eq!(svc.account_password(7).unwrap(), None);
}

#[test]
fn deleting_a_policy_removes_its_password_too() {
    let (svc, repo) = service();
    svc.save(
        McpPolicy {
            mcp_username: "reader".into(),
            ..enabled_policy(3)
        },
        AccountPassword::Set("pw".into()),
    )
    .unwrap();
    svc.delete(3).unwrap();
    assert!(repo.policies.lock().unwrap().is_empty());
    assert!(repo.passwords.lock().unwrap().is_empty());
}
