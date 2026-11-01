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

/// Told to agents so they can relay to a user how to unblock a tool.
pub const ENABLE_HINT: &str = "A user can enable a blocked tool in Schemetry under Connections > MCP policy (the global data-tools kill switch is in Settings > MCP server).";

/// Why a tool is unavailable: a stable `code` for programs, a short `reason` for people.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolBlock {
    pub code: &'static str,
    pub reason: String,
}

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
    ListInvalidObjects,
    GetObjectErrors,
}

impl McpTool {
    pub const ALL: [McpTool; 9] = [
        McpTool::ListSchemaObjects,
        McpTool::GetObjectDdl,
        McpTool::SaveMigrationScript,
        McpTool::CompareSchemas,
        McpTool::GenerateFixScript,
        McpTool::QueryTable,
        McpTool::RunQuery,
        McpTool::ListInvalidObjects,
        McpTool::GetObjectErrors,
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
            McpTool::ListInvalidObjects => "list_invalid_objects",
            McpTool::GetObjectErrors => "get_object_errors",
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

    /// Why this connection's policy itself rules `tool` out, if it does. The single
    /// source of truth for both enforcement (`check_tool`) and what agents are told
    /// (`agent_view`); the exhaustive match makes every new `McpTool` pick a rule.
    pub fn tool_block(&self, tool: McpTool) -> Option<ToolBlock> {
        if !self.mcp_enabled {
            return Some(ToolBlock {
                code: "mcp_disabled",
                reason: "MCP access is not enabled for this connection.".to_string(),
            });
        }
        let setting_off = |setting: &str| ToolBlock {
            code: "policy_disabled",
            reason: format!("disabled in this connection's MCP policy ({setting}=off)"),
        };
        let no_data_access = || ToolBlock {
            code: "data_access_none",
            reason: "data_access is none in this connection's MCP policy (needs allowlist)"
                .to_string(),
        };
        match tool {
            McpTool::ListSchemaObjects => {
                (!self.allow_schema_list).then(|| setting_off("allow_schema_list"))
            }
            McpTool::GetObjectDdl => (!self.allow_ddl_read).then(|| setting_off("allow_ddl_read")),
            McpTool::SaveMigrationScript => {
                (!self.allow_save_scripts).then(|| setting_off("allow_save_scripts"))
            }
            McpTool::CompareSchemas | McpTool::GenerateFixScript => {
                (!self.allow_compare_fix).then(|| setting_off("allow_compare_fix"))
            }
            McpTool::ListInvalidObjects => (!(self.allow_invalid_objects && self.allow_schema_list))
                .then(|| setting_off("allow_invalid_objects and allow_schema_list")),
            McpTool::GetObjectErrors => (!(self.allow_invalid_objects && self.allow_ddl_read))
                .then(|| setting_off("allow_invalid_objects and allow_ddl_read")),
            McpTool::QueryTable => {
                (self.data_access != McpDataAccess::Allowlist).then(no_data_access)
            }
            McpTool::RunQuery => {
                if self.data_access != McpDataAccess::Allowlist {
                    Some(no_data_access())
                } else {
                    (!self.allow_raw_sql).then(|| setting_off("allow_raw_sql"))
                }
            }
        }
    }

    /// Whether the policy permits `tool` at all; table/object rules are checked
    /// separately, per object.
    pub fn check_tool(&self, tool: McpTool) -> Result<(), String> {
        match self.tool_block(tool) {
            None => Ok(()),
            Some(block) if block.code == "mcp_disabled" => Err(block.reason),
            Some(block) => Err(format!(
                "{} is not permitted by this connection's MCP policy: {}.",
                tool.name(),
                block.reason
            )),
        }
    }

    /// `tool_block` plus what only matters at the point of use: the global kill switch
    /// and a data tool whose allowlist has no allow rule (so nothing can be read).
    pub fn effective_block(&self, tool: McpTool, data_tools_disabled: bool) -> Option<ToolBlock> {
        if let Some(block) = self.tool_block(tool) {
            return Some(block);
        }
        if !tool.is_data_tool() {
            return None;
        }
        if data_tools_disabled {
            return Some(ToolBlock {
                code: "kill_switch",
                reason:
                    "global kill switch on: data tools are switched off in Schemetry's settings"
                        .to_string(),
            });
        }
        if !self
            .table_rules
            .iter()
            .any(|rule| rule.action == McpRuleAction::Allow)
        {
            return Some(ToolBlock {
                code: "table_allowlist_empty",
                reason:
                    "table allowlist empty: no allow rule is configured, so no table can be read"
                        .to_string(),
            });
        }
        None
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
        self.agent_view_with(false)
    }

    /// `agent_view` given the global data-tools kill switch. Besides the original flat
    /// fields it carries `tools`: one entry per `McpTool` with `allowed` and, when not,
    /// a machine-readable `reason_code` and a human-readable `reason`.
    pub fn agent_view_with(&self, data_tools_disabled: bool) -> Value {
        let tools: serde_json::Map<String, Value> = McpTool::ALL
            .iter()
            .map(|&tool| {
                let entry = match self.effective_block(tool, data_tools_disabled) {
                    None => json!({ "allowed": true }),
                    Some(block) => json!({
                        "allowed": false,
                        "reason_code": block.code,
                        "reason": block.reason,
                    }),
                };
                (tool.name().to_string(), entry)
            })
            .collect();
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
            "invalid_objects": self.allow_invalid_objects,
            "data_access": self.data_access,
            "run_query": self.allow_raw_sql && self.data_access == McpDataAccess::Allowlist,
            "table_rules": rules,
            "max_rows": self.max_rows,
            "query_timeout_secs": self.query_timeout_secs,
            "tools": tools,
            "data_tools_disabled_globally": data_tools_disabled,
            "hint": ENABLE_HINT,
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
        // A policy without a dedicated account has no use for a stored password. This is
        // housekeeping, so it must not fail saves on machines without a keychain
        // (e.g. a headless Linux box without a Secret Service).
        if policy.mcp_username.is_empty() {
            let _ = self.repo.delete_account_password(policy.connection_id);
        }
        self.get(policy.connection_id)
    }

    /// Removes a deleted connection's policy and, best effort, its keychain entry.
    pub fn delete(&self, connection_id: i64) -> Result<()> {
        self.repo.delete(connection_id)?;
        let _ = self.repo.delete_account_password(connection_id);
        Ok(())
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
