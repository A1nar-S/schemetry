//! Per-connection MCP policy: what an agent may do, and the rule matching behind the
//! data-access allowlist. Pure logic plus a thin service over the policy repository; the
//! MCP gateway calls it before anything reaches a database.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::models::{DbType, McpDataAccess, McpPolicy, McpRuleAction, McpTableRule};
use crate::repositories::mcp_policy_repository::McpPolicyRepository;

const MAX_RULES: usize = 500;
const MAX_PATTERN_LEN: usize = 128;

/// The MCP tools that act on a connection; each maps to one policy capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpTool {
    ListSchemaObjects,
    GetObjectDdl,
    SaveMigrationScript,
    CompareSchemas,
    GenerateFixScript,
    QueryTable,
    RunQuery,
}

impl McpTool {
    pub const ALL: [McpTool; 7] = [
        McpTool::ListSchemaObjects,
        McpTool::GetObjectDdl,
        McpTool::SaveMigrationScript,
        McpTool::CompareSchemas,
        McpTool::GenerateFixScript,
        McpTool::QueryTable,
        McpTool::RunQuery,
    ];

    pub fn name(self) -> &'static str {
        match self {
            McpTool::ListSchemaObjects => "list_schema_objects",
            McpTool::GetObjectDdl => "get_object_ddl",
            McpTool::SaveMigrationScript => "save_migration_script",
            McpTool::CompareSchemas => "compare_schemas",
            McpTool::GenerateFixScript => "generate_fix_script",
            McpTool::QueryTable => "query_table",
            McpTool::RunQuery => "run_query",
        }
    }

    pub fn is_data_tool(self) -> bool {
        matches!(self, McpTool::QueryTable | McpTool::RunQuery)
    }
}

/// Glob match where `*` is any run of characters and `?` exactly one.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            backtrack = Some((p, t));
            p += 1;
        } else if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star, matched)) = backtrack {
            p = star + 1;
            t = matched + 1;
            backtrack = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn upper(value: &str) -> Vec<char> {
    value.chars().flat_map(char::to_uppercase).collect()
}

/// Case-insensitive wildcard match.
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    glob(&upper(pattern), &upper(text))
}

/// Exact-case wildcard match — Postgres identifiers are case-sensitive once quoted.
pub fn wildcard_match_exact(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    glob(&pattern, &text)
}

fn is_whole_table(rule: &McpTableRule) -> bool {
    let column = rule.column.trim();
    column.is_empty() || column == "*"
}

/// Allow rules follow the engine's identifier semantics; deny rules never do (a deny
/// must not be dodged by changing case).
fn allow_matches(pattern: &str, text: &str, db_type: DbType) -> bool {
    match db_type {
        DbType::Oracle => wildcard_match(pattern, text),
        DbType::Postgres => wildcard_match_exact(pattern, text),
    }
}

impl McpPolicy {
    /// Clamps numeric limits into their allowed range.
    pub fn normalized(mut self) -> Self {
        self.max_rows = self.max_rows.clamp(1, McpPolicy::MAX_ROWS_CEILING);
        self.query_timeout_secs = self
            .query_timeout_secs
            .clamp(1, McpPolicy::TIMEOUT_CEILING_SECS);
        self.mcp_username = self.mcp_username.trim().to_string();
        self
    }

    /// Whether the policy permits `tool` at all; table/object rules are checked
    /// separately, per object.
    pub fn check_tool(&self, tool: McpTool) -> Result<(), String> {
        if !self.mcp_enabled {
            return Err("MCP access is not enabled for this connection.".to_string());
        }
        let (allowed, setting) = match tool {
            McpTool::ListSchemaObjects => (self.allow_schema_list, "allow_schema_list"),
            McpTool::GetObjectDdl => (self.allow_ddl_read, "allow_ddl_read"),
            McpTool::SaveMigrationScript => (self.allow_save_scripts, "allow_save_scripts"),
            McpTool::CompareSchemas | McpTool::GenerateFixScript => {
                (self.allow_compare_fix, "allow_compare_fix")
            }
            McpTool::QueryTable => (
                self.data_access == McpDataAccess::Allowlist,
                "data_access = allowlist",
            ),
            McpTool::RunQuery => (
                self.data_access == McpDataAccess::Allowlist && self.allow_raw_sql,
                "data_access = allowlist and allow_raw_sql",
            ),
        };
        if allowed {
            Ok(())
        } else {
            Err(format!(
                "{} is not permitted by this connection's MCP policy ({setting} is off).",
                tool.name()
            ))
        }
    }

    /// Whether DDL/listing/compare output for this object must be hidden.
    pub fn object_denied(&self, name: &str, object_type: &str) -> bool {
        self.object_deny.iter().any(|rule| {
            let rule_type = rule.object_type.trim();
            (rule_type.is_empty()
                || rule_type == "*"
                || rule_type.eq_ignore_ascii_case(object_type.trim()))
                && wildcard_match(rule.name.trim(), name.trim())
        })
    }

    /// Table data may be read: some allow rule matches it and no deny rule covers the
    /// whole table.
    pub fn table_allowed(&self, table: &str, db_type: DbType) -> bool {
        if self.data_access != McpDataAccess::Allowlist {
            return false;
        }
        let table = table.trim();
        let denied = self.table_rules.iter().any(|rule| {
            rule.action == McpRuleAction::Deny
                && is_whole_table(rule)
                && wildcard_match(rule.table.trim(), table)
        });
        !denied
            && self.table_rules.iter().any(|rule| {
                rule.action == McpRuleAction::Allow
                    && allow_matches(rule.table.trim(), table, db_type)
            })
    }

    /// One column of an accessible table may be read.
    pub fn column_allowed(&self, table: &str, column: &str, db_type: DbType) -> bool {
        if !self.table_allowed(table, db_type) {
            return false;
        }
        let (table, column) = (table.trim(), column.trim());
        let denied = self.table_rules.iter().any(|rule| {
            rule.action == McpRuleAction::Deny
                && !is_whole_table(rule)
                && wildcard_match(rule.table.trim(), table)
                && wildcard_match(rule.column.trim(), column)
        });
        !denied
            && self.table_rules.iter().any(|rule| {
                rule.action == McpRuleAction::Allow
                    && allow_matches(rule.table.trim(), table, db_type)
                    && (is_whole_table(rule) || allow_matches(rule.column.trim(), column, db_type))
            })
    }

    /// True when any rule restricts individual columns of this table — which free-form
    /// SQL cannot honour.
    pub fn has_column_rules_for(&self, table: &str, db_type: DbType) -> bool {
        let table = table.trim();
        self.table_rules.iter().any(|rule| {
            !is_whole_table(rule)
                && match rule.action {
                    McpRuleAction::Deny => wildcard_match(rule.table.trim(), table),
                    McpRuleAction::Allow => allow_matches(rule.table.trim(), table, db_type),
                }
        })
    }

    /// The policy as an agent sees it: capabilities and limits, never the dedicated
    /// account's name.
    pub fn agent_view(&self) -> Value {
        let rules: Vec<Value> = self
            .table_rules
            .iter()
            .map(|rule| {
                json!({
                    "action": rule.action,
                    "table": rule.table,
                    "column": rule.column,
                })
            })
            .collect();
        json!({
            "list_schema_objects": self.allow_schema_list,
            "get_object_ddl": self.allow_ddl_read,
            "save_migration_script": self.allow_save_scripts,
            "compare_and_fix": self.allow_compare_fix,
            "data_access": self.data_access,
            "run_query": self.allow_raw_sql && self.data_access == McpDataAccess::Allowlist,
            "table_rules": rules,
            "max_rows": self.max_rows,
            "query_timeout_secs": self.query_timeout_secs,
        })
    }
}

fn valid_pattern(pattern: &str, allow_empty: bool) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return allow_empty;
    }
    pattern.len() <= MAX_PATTERN_LEN
        && pattern
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '#' | '*' | '?' | '-'))
}

/// Rejects policies the editor should never produce (and hand-edited rows).
pub fn validate_policy(policy: &McpPolicy) -> Result<()> {
    if policy.table_rules.len() > MAX_RULES || policy.object_deny.len() > MAX_RULES {
        bail!("Too many MCP rules (limit {MAX_RULES}).");
    }
    for rule in &policy.table_rules {
        if !valid_pattern(&rule.table, false) {
            bail!(
                "Invalid table pattern '{}': use letters, digits, _ $ # and the wildcards * ?",
                rule.table
            );
        }
        if !valid_pattern(&rule.column, true) {
            bail!(
                "Invalid column pattern '{}': use letters, digits, _ $ # and the wildcards * ?",
                rule.column
            );
        }
    }
    for rule in &policy.object_deny {
        if !valid_pattern(&rule.name, false) {
            bail!("Invalid object name pattern '{}'.", rule.name);
        }
        if !(rule.object_type.trim() == "*" || valid_pattern(&rule.object_type, true)) {
            bail!("Invalid object type '{}'.", rule.object_type);
        }
    }
    if policy.mcp_username.len() > 128 {
        bail!("The MCP database account name is too long.");
    }
    Ok(())
}

/// What to do with the dedicated account's keychain password when saving a policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountPassword {
    Keep,
    Set(String),
    Clear,
}

pub struct McpPolicyService {
    repo: Arc<dyn McpPolicyRepository>,
}

impl McpPolicyService {
    pub fn new(repo: Arc<dyn McpPolicyRepository>) -> Self {
        Self { repo }
    }

    pub fn init_db(&self) -> Result<()> {
        self.repo.init_db()
    }

    /// The connection's policy; connections without a stored one get the restrictive
    /// defaults (MCP off), which is also what existing connections see after upgrading.
    pub fn get(&self, connection_id: i64) -> Result<McpPolicy> {
        let policy = self.repo.get(connection_id)?.unwrap_or_else(|| McpPolicy {
            connection_id,
            ..McpPolicy::default()
        });
        Ok(policy.normalized())
    }

    pub fn list_all(&self) -> Result<HashMap<i64, McpPolicy>> {
        Ok(self
            .repo
            .list_all()?
            .into_iter()
            .map(|(id, policy)| (id, policy.normalized()))
            .collect())
    }

    pub fn save(&self, policy: McpPolicy, password: AccountPassword) -> Result<McpPolicy> {
        let policy = policy.normalized();
        validate_policy(&policy)?;
        self.repo.upsert(&policy)?;
        match password {
            AccountPassword::Keep => {}
            AccountPassword::Set(value) if !value.is_empty() => {
                self.repo
                    .save_account_password(policy.connection_id, &value)?;
            }
            AccountPassword::Set(_) | AccountPassword::Clear => {
                self.repo.delete_account_password(policy.connection_id)?;
            }
        }
        // A policy without a dedicated account has no use for a stored password.
        if policy.mcp_username.is_empty() {
            self.repo.delete_account_password(policy.connection_id)?;
        }
        self.get(policy.connection_id)
    }

    /// Removes a deleted connection's policy and keychain entry.
    pub fn delete(&self, connection_id: i64) -> Result<()> {
        self.repo.delete(connection_id)?;
        self.repo.delete_account_password(connection_id)
    }

    pub fn delete_all(&self) -> Result<()> {
        for id in self.repo.list_all()?.keys() {
            let _ = self.repo.delete_account_password(*id);
        }
        self.repo.delete_all()
    }

    /// The dedicated account's password, if one is stored.
    pub fn account_password(&self, connection_id: i64) -> Result<Option<String>> {
        self.repo.load_account_password(connection_id)
    }
}

#[cfg(test)]
#[path = "tests/mcp_policy.rs"]
mod tests;
