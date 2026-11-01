//! The one door agents go through: every MCP tool is implemented here as policy check →
//! database/disk work → exactly one audit entry. The rmcp layer (`crate::mcp`) is a thin
//! wrapper, which keeps all enforcement testable without a protocol client.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::models::{ConnectionRecord, DbType, Discrepancy, McpOutcome, McpPolicy, SchemaObject};
use crate::repositories::db_repository::{ReadOnlyQuery, ReadOnlyResult};
use crate::services::compare::compare_tables_across_servers;
use crate::services::connection_catalog::ConnectionCatalogService;
use crate::services::ddl_save::{self, DdlSaveArgs};
use crate::services::filter_rules::FilterRuleService;
use crate::services::fix::{self, Dialect};
use crate::services::folder_schema_overrides::SchemaFolderOverrideService;
use crate::services::mcp_audit::{McpAuditDraft, McpAuditService};
use crate::services::mcp_audit_target::{self, TARGET_KEY};
use crate::services::mcp_policy::{ENABLE_HINT, McpPolicyService, McpTool};
use crate::services::mcp_query_builder::{self, Filter, OrderBy, TableQuery};
use crate::services::mcp_sql_guard;
use crate::services::query::QueryService;
use crate::services::schema_diff::SchemaDiffService;
use crate::services::settings;
use crate::state::AppServices;

const MAX_LISTED_OBJECTS: usize = 5_000;
const MAX_DISCREPANCIES: usize = 500;
const MAX_OBJECT_ERRORS: usize = 500;
const MAX_SCRIPT_BYTES: usize = 500_000;
const MAX_RESULT_BYTES: usize = 1_000_000;
const MAX_DDL_BYTES: usize = 1_000_000;
const MAX_REASON_CHARS: usize = 2_000;
const MAX_AUDIT_ARGUMENT_BYTES: usize = 16_000;

/// Why a tool call did not produce a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolError {
    /// Refused by policy or validation; audited as `denied`.
    Denied(String),
    /// Attempted but failed; audited as `error`.
    Failed(String),
}

impl ToolError {
    pub fn message(&self) -> &str {
        match self {
            ToolError::Denied(message) | ToolError::Failed(message) => message,
        }
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::Denied(message) => write!(f, "Denied: {message}"),
            ToolError::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(error: anyhow::Error) -> Self {
        ToolError::Failed(format!("{error:#}"))
    }
}

type ToolResult = Result<Value, ToolError>;

fn denied<T>(message: impl Into<String>) -> Result<T, ToolError> {
    Err(ToolError::Denied(message.into()))
}

fn failed<T>(message: impl Into<String>) -> Result<T, ToolError> {
    Err(ToolError::Failed(message.into()))
}

macro_rules! tool_args {
    ($(#[$meta:meta])* pub struct $name:ident { $($(#[$fmeta:meta])* pub $field:ident : $ty:ty,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Deserialize, Serialize)]
        #[derive(schemars::JsonSchema)]
        pub struct $name { $($(#[$fmeta])* pub $field: $ty,)* }
    };
}

tool_args! {
    pub struct ListSchemaObjectsArgs {
        /// Id from `list_connections`.
        pub connection_id: i64,
        /// Only objects of this type, e.g. TABLE, VIEW, PACKAGE.
        #[serde(default)]
        pub object_type: Option<String>,
    }
}

tool_args! {
    pub struct GetObjectDdlArgs {
        pub connection_id: i64,
        pub object_name: String,
        /// e.g. TABLE, VIEW, PROCEDURE, FUNCTION, PACKAGE (spec only), PACKAGE BODY (body only),
        /// PACKAGE SPEC AND BODY (Oracle: spec then body, each ended by a `/` line), TRIGGER, SEQUENCE, TYPE.
        pub object_type: String,
    }
}

tool_args! {
    pub struct ListInvalidObjectsArgs {
        /// Id from `list_connections`.
        pub connection_id: i64,
        /// Only invalid objects of this type, e.g. PACKAGE BODY, VIEW.
        #[serde(default)]
        pub object_type: Option<String>,
    }
}

tool_args! {
    pub struct GetObjectErrorsArgs {
        pub connection_id: i64,
        pub object_name: String,
        /// e.g. VIEW, PROCEDURE, FUNCTION, PACKAGE, PACKAGE BODY, TRIGGER, TYPE, TYPE BODY.
        pub object_type: String,
    }
}

tool_args! {
    pub struct SaveMigrationScriptArgs {
        pub connection_id: i64,
        pub object_name: String,
        /// e.g. TABLE, VIEW, PACKAGE, PACKAGE BODY, or PACKAGE SPEC AND BODY (Oracle: one .pck file
        /// holding both, so `ddl` must contain CREATE PACKAGE and CREATE PACKAGE BODY).
        pub object_type: String,
        /// The DDL to save.
        pub ddl: String,
        /// Short label used in the migration file name: letters, digits, `_` and `-`, up to 80 characters.
        pub description: String,
        /// Return the target paths and rendered content without writing anything.
        #[serde(default)]
        pub dry_run: bool,
    }
}

tool_args! {
    pub struct CompareSchemasArgs {
        /// The reference connection.
        pub source_id: i64,
        /// Connections to compare against the reference (same database engine).
        pub target_ids: Vec<i64>,
        /// Report missing tables (default true).
        #[serde(default)]
        pub check_tables: Option<bool>,
        /// Report missing columns and type/length/default differences (default true).
        #[serde(default)]
        pub check_columns: Option<bool>,
        /// Report comment differences (default false).
        #[serde(default)]
        pub check_comments: Option<bool>,
        /// Report index differences (default false).
        #[serde(default)]
        pub check_indexes: Option<bool>,
    }
}

tool_args! {
    pub struct GenerateFixScriptArgs {
        pub source_id: i64,
        pub target_ids: Vec<i64>,
        /// Only fix these tables; omit for every discrepancy.
        #[serde(default)]
        pub tables: Option<Vec<String>>,
        #[serde(default)]
        pub check_tables: Option<bool>,
        #[serde(default)]
        pub check_columns: Option<bool>,
        #[serde(default)]
        pub check_comments: Option<bool>,
        #[serde(default)]
        pub check_indexes: Option<bool>,
    }
}

tool_args! {
    pub struct QueryTableArgs {
        pub connection_id: i64,
        pub table: String,
        /// Columns to return; omit for every column the policy allows.
        #[serde(default)]
        pub columns: Option<Vec<String>>,
        /// Conditions combined with AND.
        #[serde(default)]
        pub filters: Option<Vec<Filter>>,
        #[serde(default)]
        pub order_by: Option<Vec<OrderBy>>,
        /// Row limit; capped by the connection's `max_rows`.
        #[serde(default)]
        pub limit: Option<u32>,
    }
}

tool_args! {
    pub struct RunQueryArgs {
        pub connection_id: i64,
        /// A single SELECT statement.
        pub sql: String,
        /// Row limit; capped by the connection's `max_rows`.
        #[serde(default)]
        pub max_rows: Option<u32>,
    }
}

/// Per-call state the audit entry is built from.
#[derive(Default)]
struct Ctx {
    connection: Option<(i64, String)>,
    details: Option<Value>,
    /// What the call was about (tables, objects, …); stored under `arguments.target`.
    target: Option<Map<String, Value>>,
    row_count: Option<i64>,
}

struct Resolved {
    conn: ConnectionRecord,
    policy: McpPolicy,
}

/// The slice of global app settings the gateway consults; a trait so tests can supply
/// their own instead of touching the shared SQLite file.
pub trait GatewaySettings: Send + Sync {
    /// The global kill switch for data tools.
    fn data_tools_disabled(&self) -> bool;
    fn schema_root(&self) -> Result<String, String>;
    fn save_options(&self, object_type: &str) -> ddl_save::DdlSaveOptions;
}

/// Reads the real settings.
pub struct AppSettings;

impl GatewaySettings for AppSettings {
    fn data_tools_disabled(&self) -> bool {
        settings::load_mcp_data_tools_disabled()
    }

    fn schema_root(&self) -> Result<String, String> {
        ddl_save::schema_root_from_settings()
    }

    fn save_options(&self, object_type: &str) -> ddl_save::DdlSaveOptions {
        ddl_save::options_from_settings(object_type)
    }
}

pub struct McpGateway {
    catalog: Arc<ConnectionCatalogService>,
    diff_svc: Arc<SchemaDiffService>,
    query_svc: Arc<QueryService>,
    filter_rules_svc: Arc<FilterRuleService>,
    folder_overrides: Arc<SchemaFolderOverrideService>,
    policy_svc: Arc<McpPolicyService>,
    audit_svc: Arc<McpAuditService>,
    settings: Arc<dyn GatewaySettings>,
}

/// The schema a connection's objects live in.
pub fn schema_of(conn: &ConnectionRecord) -> String {
    match conn.db_type {
        DbType::Postgres => {
            let schema = conn.pg_schema.trim();
            if schema.is_empty() { "public" } else { schema }.to_string()
        }
        DbType::Oracle => conn.username.to_ascii_uppercase(),
    }
}

fn db_type_name(db_type: DbType) -> &'static str {
    match db_type {
        DbType::Oracle => "oracle",
        DbType::Postgres => "postgres",
    }
}

fn unavailable(connection_id: i64) -> String {
    format!("Connection {connection_id} is not available through MCP.")
}

/// File-name component rules for anything an agent names: no separators, dots or spaces,
/// so nothing can climb out of the schema folder.
fn valid_file_component(value: &str, max_len: usize, allow_special: bool) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || c == '_'
                || c == '-'
                || (allow_special && matches!(c, '$' | '#'))
        })
}

fn valid_object_type(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 30
        && trimmed.chars().all(|c| c.is_ascii_alphabetic() || c == ' ')
}

const PACKAGE_COMBINED_ORACLE_ONLY: &str =
    "PACKAGE SPEC AND BODY applies to Oracle packages only; PostgreSQL has no packages.";

/// Hiding rules apply per type, so the combined package type is hidden when either half
/// (or the combined type itself) is.
fn object_hidden(policy: &McpPolicy, name: &str, object_type: &str) -> bool {
    if ddl_save::is_package_spec_and_body(object_type) {
        ["PACKAGE", "PACKAGE BODY", object_type]
            .iter()
            .any(|t| policy.object_denied(name, t))
    } else {
        policy.object_denied(name, object_type)
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The audit log is append-only forever, so agent-supplied text must not be able to bloat
/// it: oversized arguments are replaced by a preview, their size and a digest.
fn bounded_arguments(arguments: Value) -> String {
    let text = arguments.to_string();
    if text.len() <= MAX_AUDIT_ARGUMENT_BYTES {
        return text;
    }
    let mut replacement = json!({
        "truncated": true,
        "original_bytes": text.len(),
        "sha256": sha256_hex(text.as_bytes()),
        "preview": truncate_chars(&text, 2_000),
    });
    // The target summary is itself bounded, so it always survives.
    if let Some(target) = arguments.get(TARGET_KEY) {
        replacement[TARGET_KEY] = target.clone();
    }
    replacement.to_string()
}

fn target_of(object_type: &str, names: &[String]) -> Map<String, Value> {
    mcp_audit_target::object_target(object_type, names)
}

impl McpGateway {
    pub fn new(services: &AppServices) -> Self {
        Self::from_parts(
            Arc::clone(&services.catalog),
            Arc::clone(&services.diff_svc),
            Arc::clone(&services.query_svc),
            Arc::clone(&services.filter_rules_svc),
            Arc::clone(&services.folder_schema_overrides_svc),
            Arc::clone(&services.mcp_policy_svc),
            Arc::clone(&services.mcp_audit_svc),
            Arc::new(AppSettings),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_parts(
        catalog: Arc<ConnectionCatalogService>,
        diff_svc: Arc<SchemaDiffService>,
        query_svc: Arc<QueryService>,
        filter_rules_svc: Arc<FilterRuleService>,
        folder_overrides: Arc<SchemaFolderOverrideService>,
        policy_svc: Arc<McpPolicyService>,
        audit_svc: Arc<McpAuditService>,
        settings: Arc<dyn GatewaySettings>,
    ) -> Self {
        Self {
            catalog,
            diff_svc,
            query_svc,
            filter_rules_svc,
            folder_overrides,
            policy_svc,
            audit_svc,
            settings,
        }
    }

    // ── plumbing ────────────────────────────────────────────────────────────────

    /// Runs `f` and records exactly one audit entry for it, whatever happens. If the
    /// entry cannot be written the result is withheld (fail closed).
    fn audited(
        &self,
        client: &str,
        tool: &str,
        connection_id: Option<i64>,
        arguments: Value,
        f: impl FnOnce(&mut Ctx) -> ToolResult,
    ) -> ToolResult {
        let started = Instant::now();
        let mut ctx = Ctx {
            connection: connection_id.map(|id| (id, String::new())),
            ..Ctx::default()
        };
        let result = f(&mut ctx);

        let (outcome, reason, bytes) = match &result {
            Ok(value) => (
                McpOutcome::Allowed,
                String::new(),
                value.to_string().len() as i64,
            ),
            Err(ToolError::Denied(message)) => (McpOutcome::Denied, message.clone(), 0),
            Err(ToolError::Failed(message)) => (McpOutcome::Error, message.clone(), 0),
        };

        let mut arguments = arguments;
        if let (Some(Value::Object(extra)), Value::Object(map)) =
            (ctx.details.take(), &mut arguments)
        {
            for (key, value) in extra {
                map.insert(key, value);
            }
        }
        if let (Some(target), Value::Object(map)) = (ctx.target.take(), &mut arguments) {
            map.insert(TARGET_KEY.to_string(), Value::Object(target));
        }
        let (connection_id, connection_name) = match ctx.connection {
            Some((id, name)) => (Some(id), name),
            None => (None, String::new()),
        };

        let draft = McpAuditDraft {
            tool: tool.to_string(),
            connection_id,
            connection_name,
            arguments: bounded_arguments(arguments),
            outcome,
            reason: truncate_chars(&reason, MAX_REASON_CHARS),
            row_count: ctx.row_count,
            bytes,
            duration_ms: started.elapsed().as_millis() as i64,
            client_name: truncate_chars(client, 200),
        };
        match self.audit_svc.record(draft) {
            Ok(_) => result,
            Err(error) => failed(format!(
                "The audit log could not be written, so the result was withheld: {error:#}"
            )),
        }
    }

    /// Finds the connection and applies the policy gates before any credential is read
    /// from the keychain.
    fn resolve(
        &self,
        ctx: &mut Ctx,
        connection_id: i64,
        tool: McpTool,
    ) -> Result<Resolved, ToolError> {
        let summary = self
            .catalog
            .list_connection_summaries()?
            .into_iter()
            .find(|c| c.id == connection_id);
        let Some(summary) = summary else {
            return denied(unavailable(connection_id));
        };
        ctx.connection = Some((summary.id, summary.name.clone()));

        let policy = self.policy_svc.get(connection_id)?;
        if !policy.mcp_enabled {
            // Same wording as an unknown id: don't reveal which connections exist.
            return denied(unavailable(connection_id));
        }
        policy.check_tool(tool).map_err(ToolError::Denied)?;
        if tool.is_data_tool() && self.settings.data_tools_disabled() {
            return denied("MCP data tools are switched off globally in Schemetry's settings.");
        }

        let conn = self
            .catalog
            .get_connection(connection_id)?
            .ok_or_else(|| ToolError::Denied(unavailable(connection_id)))?;
        if conn.password_broken {
            return failed(
                "The stored password for this connection could not be read from the OS keychain.",
            );
        }
        Ok(Resolved { conn, policy })
    }

    fn blocking_filter_rules(&self) -> Result<Vec<crate::models::TableFilterRule>, ToolError> {
        Ok(self.filter_rules_svc.list_active_rules()?)
    }

    // ── tools ───────────────────────────────────────────────────────────────────

    pub fn list_connections(&self, client: &str) -> ToolResult {
        self.audited(client, "list_connections", None, json!({}), |ctx| {
            let policies = self.policy_svc.list_all()?;
            let kill_switch = self.settings.data_tools_disabled();
            let mut out = Vec::new();
            for conn in self.catalog.list_connection_summaries()? {
                let Some(policy) = policies.get(&conn.id).filter(|p| p.mcp_enabled) else {
                    continue;
                };
                out.push(json!({
                    "id": conn.id,
                    "name": conn.name,
                    "group": conn.group_name,
                    "db_type": db_type_name(conn.db_type),
                    "schema": schema_of(&conn),
                    "policy": policy.agent_view_with(kill_switch),
                }));
            }
            ctx.row_count = Some(out.len() as i64);
            Ok(json!({
                "connections": out,
                "data_tools_disabled_globally": kill_switch,
                "hint": ENABLE_HINT,
            }))
        })
    }

    pub fn list_schema_objects(&self, client: &str, args: ListSchemaObjectsArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::ListSchemaObjects.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                let Resolved { conn, policy } =
                    self.resolve(ctx, args.connection_id, McpTool::ListSchemaObjects)?;
                let rules = self.blocking_filter_rules()?;
                let wanted = args
                    .object_type
                    .as_deref()
                    .map(|t| t.trim().to_ascii_uppercase())
                    .filter(|t| !t.is_empty());

                let objects: Vec<SchemaObject> = self
                    .diff_svc
                    .fetch_schema_objects(&conn, &rules)?
                    .into_iter()
                    .filter(|o| !policy.object_denied(&o.name, &o.object_type))
                    .filter(|o| {
                        wanted
                            .as_ref()
                            .is_none_or(|w| o.object_type.eq_ignore_ascii_case(w))
                    })
                    .collect();

                let total = objects.len();
                ctx.row_count = Some(total as i64);
                let listed: Vec<&SchemaObject> = objects.iter().take(MAX_LISTED_OBJECTS).collect();
                Ok(json!({
                    "objects": listed,
                    "count": total,
                    "truncated": total > MAX_LISTED_OBJECTS,
                }))
            },
        )
    }

    pub fn get_object_ddl(&self, client: &str, args: GetObjectDdlArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::GetObjectDdl.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                ctx.target = Some(target_of(
                    &args.object_type.trim().to_ascii_uppercase(),
                    std::slice::from_ref(&args.object_name),
                ));
                let Resolved { conn, policy } =
                    self.resolve(ctx, args.connection_id, McpTool::GetObjectDdl)?;
                let name = args.object_name.trim();
                let object_type = args.object_type.trim().to_ascii_uppercase();
                if !mcp_query_builder::valid_identifier(name) {
                    return denied("object_name must be a plain identifier.");
                }
                if !valid_object_type(&object_type) {
                    return denied("object_type is not a recognised object type.");
                }
                if ddl_save::is_package_spec_and_body(&object_type) && conn.db_type != DbType::Oracle {
                    return denied(PACKAGE_COMBINED_ORACLE_ONLY);
                }
                if object_hidden(&policy, name, &object_type) {
                    return denied(format!("DDL for {object_type} {name} is hidden by policy."));
                }

                let (ddl, includes): (String, &[&str]) = match object_type.as_str() {
                    ddl_save::PACKAGE_SPEC_AND_BODY => {
                        let spec = self.diff_svc.fetch_object_ddl(&conn, name, "PACKAGE")?;
                        let body = self
                            .diff_svc
                            .fetch_object_ddl(&conn, name, "PACKAGE BODY")
                            .map_err(|e| {
                                ToolError::Failed(format!(
                                    "Could not read the body of package {name}: {e}. \
                                     If it has no body, request object_type PACKAGE."
                                ))
                            })?;
                        (
                            ddl_save::combine_package_spec_and_body(&spec, &body),
                            &["spec", "body"],
                        )
                    }
                    other => {
                        let ddl = self.diff_svc.fetch_object_ddl(&conn, name, other)?;
                        let includes: &[&str] = match other {
                            "PACKAGE" => &["spec"],
                            "PACKAGE BODY" => &["body"],
                            _ => &[],
                        };
                        (ddl, includes)
                    }
                };
                ctx.row_count = Some(1);
                let mut result = json!({ "object_name": name, "object_type": object_type, "ddl": ddl });
                if !includes.is_empty() {
                    result["includes"] = json!(includes);
                }
                Ok(result)
            },
        )
    }

    pub fn list_invalid_objects(&self, client: &str, args: ListInvalidObjectsArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::ListInvalidObjects.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                let Resolved { conn, policy } =
                    self.resolve(ctx, args.connection_id, McpTool::ListInvalidObjects)?;
                let wanted = args
                    .object_type
                    .as_deref()
                    .map(|t| t.trim().to_ascii_uppercase())
                    .filter(|t| !t.is_empty());
                if wanted.as_deref().is_some_and(|t| !valid_object_type(t)) {
                    return denied("object_type is not a recognised object type.");
                }

                let list = self.diff_svc.fetch_invalid_objects(
                    &conn,
                    wanted.as_deref(),
                    MAX_LISTED_OBJECTS,
                )?;
                let fetched = list.objects.len();
                let objects: Vec<_> = list
                    .objects
                    .into_iter()
                    .filter(|o| !policy.object_denied(&o.name, &o.object_type))
                    .collect();

                ctx.row_count = Some(objects.len() as i64);
                ctx.details = Some(json!({
                    "invalid_count": objects.len(),
                    "hidden_by_policy": fetched - objects.len(),
                }));
                Ok(json!({
                    "applicable": list.applicable,
                    "message": list.message,
                    "objects": objects,
                    "count": objects.len(),
                    "truncated": list.truncated,
                }))
            },
        )
    }

    pub fn get_object_errors(&self, client: &str, args: GetObjectErrorsArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::GetObjectErrors.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                let Resolved { conn, policy } =
                    self.resolve(ctx, args.connection_id, McpTool::GetObjectErrors)?;
                let name = args.object_name.trim();
                let object_type = args.object_type.trim().to_ascii_uppercase();
                if !mcp_query_builder::valid_identifier(name) {
                    return denied("object_name must be a plain identifier.");
                }
                if !valid_object_type(&object_type) {
                    return denied("object_type is not a recognised object type.");
                }
                if policy.object_denied(name, &object_type) {
                    return denied(format!(
                        "Errors for {object_type} {name} are hidden by policy."
                    ));
                }

                let list = self.diff_svc.fetch_object_errors(
                    &conn,
                    name,
                    &object_type,
                    MAX_OBJECT_ERRORS,
                )?;
                let warnings = list
                    .errors
                    .iter()
                    .filter(|e| e.attribute == "WARNING")
                    .count();
                let errors = list.errors.len() - warnings;
                ctx.row_count = Some(list.errors.len() as i64);
                ctx.details = Some(json!({ "error_count": errors, "warning_count": warnings }));
                Ok(json!({
                    "object_name": name,
                    "object_type": object_type,
                    "applicable": list.applicable,
                    "message": list.message,
                    "status": list.status,
                    "errors": list.errors,
                    "error_count": errors,
                    "warning_count": warnings,
                    "truncated": list.truncated,
                }))
            },
        )
    }

    pub fn save_migration_script(&self, client: &str, args: SaveMigrationScriptArgs) -> ToolResult {
        // The DDL itself can be large; the audit trail keeps its size and digest.
        let arguments = json!({
            "connection_id": args.connection_id,
            "object_name": args.object_name,
            "object_type": args.object_type,
            "description": args.description,
            "dry_run": args.dry_run,
            "ddl_bytes": args.ddl.len(),
            "ddl_sha256": sha256_hex(args.ddl.as_bytes()),
        });
        self.audited(
            client,
            McpTool::SaveMigrationScript.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                ctx.target = Some(target_of(
                    &args.object_type.trim().to_ascii_uppercase(),
                    std::slice::from_ref(&args.object_name),
                ));
                let Resolved { conn, policy } =
                    self.resolve(ctx, args.connection_id, McpTool::SaveMigrationScript)?;

                let object_name = args.object_name.trim();
                let object_type = args.object_type.trim().to_ascii_uppercase();
                let description = args.description.trim();
                if !valid_file_component(object_name, 128, true) {
                    return denied(
                        "object_name may only contain letters, digits, _ - $ # (no paths or dots).",
                    );
                }
                if !valid_object_type(&object_type) {
                    return denied("object_type is not a recognised object type.");
                }
                if !valid_file_component(description, 80, false) {
                    return denied(
                        "description may only contain letters, digits, _ and - (max 80 characters).",
                    );
                }
                if args.ddl.trim().is_empty() || args.ddl.len() > MAX_DDL_BYTES || args.ddl.contains('\0') {
                    return denied("ddl must be non-empty text of at most 1 MB.");
                }
                if object_hidden(&policy, object_name, &object_type) {
                    return denied(format!("{object_type} {object_name} is hidden by policy."));
                }
                if ddl_save::is_package_spec_and_body(&object_type) {
                    if conn.db_type != DbType::Oracle {
                        return denied(PACKAGE_COMBINED_ORACLE_ONLY);
                    }
                    if !ddl_save::has_package_spec_and_body(&args.ddl) {
                        return denied(
                            "ddl for PACKAGE SPEC AND BODY must contain both a CREATE PACKAGE and a CREATE PACKAGE BODY statement.",
                        );
                    }
                }

                let schema_root = self.settings.schema_root().map_err(ToolError::Failed)?;
                let save_args = DdlSaveArgs {
                    schema: schema_of(&conn),
                    object_name: object_name.to_string(),
                    object_type: object_type.clone(),
                    ddl: args.ddl.clone(),
                    description: description.to_string(),
                    db_type: db_type_name(conn.db_type).to_string(),
                };
                let options = self.settings.save_options(&object_type);
                let plan =
                    ddl_save::plan_ddl_save(&self.folder_overrides, &schema_root, &save_args, &options)
                        .map_err(ToolError::Failed)?;

                // Names were validated, so every target already sits inside the schema folder;
                // this guards the join result itself.
                let schema_dir = ddl_save::resolve_schema_dir(&self.folder_overrides, &save_args.schema, &schema_root)
                    .map_err(ToolError::Failed)?;
                for file in &plan.files {
                    let escapes = file
                        .path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                        || !file.path.starts_with(&schema_dir);
                    if escapes {
                        return denied("The target path would leave the configured schema folder.");
                    }
                }

                let paths: Vec<String> = plan
                    .files
                    .iter()
                    .map(|f| f.path.to_string_lossy().into_owned())
                    .collect();
                ctx.details = Some(json!({ "target_paths": paths }));
                ctx.row_count = Some(plan.files.len() as i64);

                let files: Vec<Value> = plan
                    .files
                    .iter()
                    .map(|file| {
                        let mut entry = json!({
                            "kind": file.kind,
                            "path": file.path.to_string_lossy(),
                            "overwrites_existing": file.exists,
                        });
                        if args.dry_run {
                            entry["content"] =
                                Value::String(String::from_utf8_lossy(&file.content).into_owned());
                        }
                        entry
                    })
                    .collect();

                if args.dry_run {
                    return Ok(json!({ "dry_run": true, "written": false, "files": files }));
                }
                ddl_save::write_plan(&plan).map_err(ToolError::Failed)?;
                Ok(json!({ "dry_run": false, "written": true, "files": files }))
            },
        )
    }

    /// Resolves every connection of a compare/fix request and fetches the schemas.
    #[allow(clippy::type_complexity)]
    fn load_comparison(
        &self,
        ctx: &mut Ctx,
        tool: McpTool,
        source_id: i64,
        target_ids: &[i64],
        flags: (bool, bool, bool, bool),
    ) -> Result<Comparison, ToolError> {
        if target_ids.is_empty() {
            return denied("target_ids must list at least one connection.");
        }
        if target_ids.len() > 20 {
            return denied("At most 20 target connections can be compared at once.");
        }
        let mut ids = vec![source_id];
        for id in target_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }

        let mut resolved = Vec::new();
        for id in &ids {
            resolved.push(self.resolve(ctx, *id, tool)?);
        }
        // Several connections are involved; the audit entry names the reference one.
        ctx.connection = Some((source_id, resolved[0].conn.name.clone()));
        let server_names: Vec<String> = resolved.iter().map(|r| r.conn.name.clone()).collect();
        let target = ctx
            .target
            .get_or_insert_with(|| target_of("SCHEMA", &server_names));
        target.insert("connections".into(), json!(server_names));

        if resolved
            .iter()
            .any(|r| r.conn.db_type != resolved[0].conn.db_type)
        {
            return failed("Cannot compare servers from different database engines.");
        }

        let connections: Vec<ConnectionRecord> = resolved.iter().map(|r| r.conn.clone()).collect();
        let rules = self.blocking_filter_rules()?;
        let (servers, errors) = self.diff_svc.fetch_from_connections(&connections, &rules);
        if !errors.is_empty() {
            let mut messages: Vec<String> = errors
                .iter()
                .map(|(id, error)| {
                    let name = connections
                        .iter()
                        .find(|c| c.id == *id)
                        .map(|c| c.name.as_str())
                        .unwrap_or("?");
                    format!("{name}: {error}")
                })
                .collect();
            messages.sort();
            return failed(format!("Could not read schemas — {}", messages.join("; ")));
        }

        let names: HashMap<i64, String> =
            connections.iter().map(|c| (c.id, c.name.clone())).collect();
        let (tables, columns, comments, indexes) = flags;
        let all = compare_tables_across_servers(
            &servers, &names, source_id, tables, columns, comments, indexes,
        )?;

        let policies: Vec<&McpPolicy> = resolved.iter().map(|r| &r.policy).collect();
        let before = all.len();
        let discrepancies: Vec<Discrepancy> = all
            .into_iter()
            .filter(|d| {
                !policies
                    .iter()
                    .any(|p| p.object_denied(&d.table_name, "TABLE"))
            })
            .collect();
        let hidden = before - discrepancies.len();

        Ok(Comparison {
            connections,
            servers,
            names,
            discrepancies,
            hidden,
        })
    }

    pub fn compare_schemas(&self, client: &str, args: CompareSchemasArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::CompareSchemas.name(),
            Some(args.source_id),
            arguments,
            |ctx| {
                let flags = (
                    args.check_tables.unwrap_or(true),
                    args.check_columns.unwrap_or(true),
                    args.check_comments.unwrap_or(false),
                    args.check_indexes.unwrap_or(false),
                );
                let cmp = self.load_comparison(
                    ctx,
                    McpTool::CompareSchemas,
                    args.source_id,
                    &args.target_ids,
                    flags,
                )?;
                let total = cmp.discrepancies.len();
                ctx.row_count = Some(total as i64);
                let listed: Vec<Value> = cmp
                    .discrepancies
                    .iter()
                    .take(MAX_DISCREPANCIES)
                    .map(|d| {
                        json!({
                            "difference": d.difference,
                            "element": d.element,
                            "table": d.table_name,
                            "column": d.column_name,
                            "server_id": d.server_id,
                            "server": d.server_name,
                            "details": d.details,
                        })
                    })
                    .collect();
                Ok(json!({
                    "discrepancies": listed,
                    "count": total,
                    "truncated": total > MAX_DISCREPANCIES,
                    "hidden_by_policy": cmp.hidden,
                }))
            },
        )
    }

    pub fn generate_fix_script(&self, client: &str, args: GenerateFixScriptArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::GenerateFixScript.name(),
            Some(args.source_id),
            arguments,
            |ctx| {
                if let Some(tables) = args.tables.as_ref().filter(|t| !t.is_empty()) {
                    ctx.target = Some(target_of("TABLE", tables));
                }
                let flags = (
                    args.check_tables.unwrap_or(true),
                    args.check_columns.unwrap_or(true),
                    args.check_comments.unwrap_or(false),
                    args.check_indexes.unwrap_or(false),
                );
                let cmp = self.load_comparison(
                    ctx,
                    McpTool::GenerateFixScript,
                    args.source_id,
                    &args.target_ids,
                    flags,
                )?;

                let only: Option<HashSet<String>> = args
                    .tables
                    .as_ref()
                    .map(|t| t.iter().map(|n| n.trim().to_ascii_uppercase()).collect());
                let selected: HashSet<usize> = cmp
                    .discrepancies
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| {
                        only.as_ref().is_none_or(|set| {
                            set.contains(&d.table_name.trim().to_ascii_uppercase())
                        })
                    })
                    .map(|(i, _)| i)
                    .collect();
                if selected.is_empty() {
                    return failed("There are no discrepancies to fix for this selection.");
                }

                let missing: Vec<String> = {
                    let mut set = HashSet::new();
                    for index in &selected {
                        let d = &cmp.discrepancies[*index];
                        if d.difference.eq_ignore_ascii_case("MISSING")
                            && d.column_name.trim().is_empty()
                            && d.server_id != args.source_id
                        {
                            set.insert(d.table_name.trim().to_ascii_uppercase());
                        }
                    }
                    set.into_iter().collect()
                };
                let mut table_ddls = HashMap::new();
                if !missing.is_empty() {
                    let source = cmp
                        .connections
                        .iter()
                        .find(|c| c.id == args.source_id)
                        .ok_or_else(|| {
                            ToolError::Failed("Reference connection not loaded.".into())
                        })?;
                    table_ddls.insert(
                        args.source_id,
                        self.diff_svc
                            .fetch_table_ddls_for_tables(source, &missing)?,
                    );
                }

                let dialects: HashMap<i64, Dialect> = cmp
                    .connections
                    .iter()
                    .map(|c| {
                        (
                            c.id,
                            match c.db_type {
                                DbType::Oracle => Dialect::Oracle,
                                DbType::Postgres => Dialect::Postgres,
                            },
                        )
                    })
                    .collect();
                let result = fix::generate_fix_script(
                    &cmp.discrepancies,
                    &selected,
                    &cmp.servers,
                    &table_ddls,
                    args.source_id,
                    &cmp.names,
                    &dialects,
                )?;

                let truncated = result.script.len() > MAX_SCRIPT_BYTES;
                let script = if truncated {
                    let mut end = MAX_SCRIPT_BYTES;
                    while !result.script.is_char_boundary(end) {
                        end -= 1;
                    }
                    result.script[..end].to_string()
                } else {
                    result.script.clone()
                };
                ctx.row_count = Some(result.generated_count as i64);
                Ok(json!({
                    "script": script,
                    "generated_count": result.generated_count,
                    "skipped_count": result.skipped_count,
                    "truncated": truncated,
                    "hidden_by_policy": cmp.hidden,
                }))
            },
        )
    }

    pub fn query_table(&self, client: &str, args: QueryTableArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::QueryTable.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                let mut target = target_of("TABLE", std::slice::from_ref(&args.table));
                target.insert(
                    "filters".into(),
                    json!(args.filters.as_ref().map_or(0, Vec::len)),
                );
                ctx.target = Some(target);
                let resolved = self.resolve(ctx, args.connection_id, McpTool::QueryTable)?;
                let Resolved { conn, policy } = &resolved;
                let table = args.table.trim();
                if let Some(target) = ctx.target.as_mut() {
                    let named = target_of("TABLE", &[actual_table_name(table, conn.db_type)]);
                    target.insert("names".into(), named["names"].clone());
                }
                if !mcp_query_builder::valid_identifier(table) {
                    return denied("table must be a plain table name.");
                }
                if !policy.table_allowed(table, conn.db_type) {
                    return denied(format!("Reading table data from {table} is not permitted."));
                }

                let actual = self.query_svc.table_columns(conn, table)?;
                if actual.is_empty() {
                    return failed(format!(
                        "Table {table} was not found or has no visible columns."
                    ));
                }
                let canonical = |name: &str| -> Result<String, ToolError> {
                    let name = name.trim();
                    actual
                        .iter()
                        .find(|c| c.as_str() == name)
                        .or_else(|| actual.iter().find(|c| c.eq_ignore_ascii_case(name)))
                        .cloned()
                        .map_or_else(|| failed(format!("Unknown column {name} on {table}.")), Ok)
                };
                let readable = |column: &str| policy.column_allowed(table, column, conn.db_type);

                let mut omitted = 0usize;
                let selected: Vec<String> = match &args.columns {
                    Some(requested) if !requested.is_empty() => {
                        let mut cols = Vec::new();
                        for name in requested {
                            let column = canonical(name)?;
                            if !readable(&column) {
                                return denied(format!(
                                    "Reading column {table}.{column} is not permitted."
                                ));
                            }
                            cols.push(column);
                        }
                        cols
                    }
                    _ => {
                        let allowed: Vec<String> =
                            actual.iter().filter(|c| readable(c)).cloned().collect();
                        omitted = actual.len() - allowed.len();
                        allowed
                    }
                };
                if selected.is_empty() {
                    return denied(format!("No column of {table} may be read."));
                }

                let mut filters = args.filters.clone().unwrap_or_default();
                for filter in &mut filters {
                    let column = canonical(&filter.column)?;
                    if !readable(&column) {
                        return denied(format!("Filtering on {table}.{column} is not permitted."));
                    }
                    filter.column = column;
                }
                let mut order_by = args.order_by.clone().unwrap_or_default();
                for order in &mut order_by {
                    let column = canonical(&order.column)?;
                    if !readable(&column) {
                        return denied(format!("Ordering by {table}.{column} is not permitted."));
                    }
                    order.column = column;
                }

                let table_name = actual_table_name(table, conn.db_type);
                let limit = args
                    .limit
                    .unwrap_or(policy.max_rows)
                    .clamp(1, policy.max_rows);
                let schema = schema_of(conn);
                let built = mcp_query_builder::build(
                    &TableQuery {
                        schema: &schema,
                        table: &table_name,
                        columns: &selected,
                        filters: &filters,
                        order_by: &order_by,
                        limit,
                    },
                    conn.db_type,
                )
                .map_err(ToolError::Denied)?;
                ctx.details = Some(json!({ "sql": built.sql, "binds": built.binds }));
                if let Some(target) = ctx.target.as_mut() {
                    mcp_audit_target::set_columns(target, &selected);
                    target.insert("limit".into(), json!(limit));
                }

                let request = ReadOnlyQuery {
                    sql: built.sql,
                    binds: built.binds,
                    max_rows: limit,
                    timeout_secs: policy.query_timeout_secs,
                };
                let result = self.run_readonly(&resolved, &request)?;
                ctx.row_count = Some(result.rows.len() as i64);
                let mut out = rows_to_json(result, limit);
                note_truncated(ctx, &out);
                if omitted > 0 {
                    out["omitted_columns"] = json!(omitted);
                    out["note"] = json!(
                        "Some columns were left out because the policy does not allow reading them."
                    );
                }
                Ok(out)
            },
        )
    }

    pub fn run_query(&self, client: &str, args: RunQueryArgs) -> ToolResult {
        let arguments = serde_json::to_value(&args).unwrap_or_default();
        self.audited(
            client,
            McpTool::RunQuery.name(),
            Some(args.connection_id),
            arguments,
            |ctx| {
                let resolved = self.resolve(ctx, args.connection_id, McpTool::RunQuery)?;
                let Resolved { conn, policy } = &resolved;

                let analyzed = mcp_sql_guard::analyze(&args.sql, conn.db_type).map_err(ToolError::Denied)?;
                ctx.target = Some(target_of("TABLE", &analyzed.table_names()));
                let own_schema = schema_of(conn);
                for table in &analyzed.tables {
                    if let Some(schema) = &table.schema
                        && !schema.eq_ignore_ascii_case(&own_schema)
                    {
                        return denied(format!(
                            "Only tables in the connection's own schema ({own_schema}) can be queried."
                        ));
                    }
                    if !policy.table_allowed(&table.name, conn.db_type) {
                        return denied(format!("Reading table data from {} is not permitted.", table.name));
                    }
                    if policy.has_column_rules_for(&table.name, conn.db_type) {
                        return denied(format!(
                            "Column-level rules apply to {}; raw SQL cannot honour them — use query_table.",
                            table.name
                        ));
                    }
                    // Catalog views and synonyms resolve outside the connection's own
                    // schema, so a table must actually live in it (this also keeps broad
                    // `allow *` rules from opening pg_catalog / ALL_* / DBA_* views).
                    if self.query_svc.table_columns(conn, &table.name)?.is_empty() {
                        return denied(format!(
                            "{} is not a table or view in schema {own_schema}.",
                            table.name
                        ));
                    }
                }

                let limit = args.max_rows.unwrap_or(policy.max_rows).clamp(1, policy.max_rows);
                let request = ReadOnlyQuery {
                    sql: analyzed.sql,
                    binds: Vec::new(),
                    max_rows: limit,
                    timeout_secs: policy.query_timeout_secs,
                };
                let result = self.run_readonly(&resolved, &request)?;
                ctx.row_count = Some(result.rows.len() as i64);
                let out = rows_to_json(result, limit);
                if let Some(target) = ctx.target.as_mut() {
                    target.insert("limit".into(), json!(limit));
                }
                note_truncated(ctx, &out);
                Ok(out)
            },
        )
    }

    /// Runs on the dedicated MCP account when one is configured, otherwise on the
    /// connection's own credentials.
    fn run_readonly(
        &self,
        resolved: &Resolved,
        request: &ReadOnlyQuery,
    ) -> Result<ReadOnlyResult, ToolError> {
        let mut conn = resolved.conn.clone();
        if !resolved.policy.mcp_username.is_empty() {
            let Some(password) = self.policy_svc.account_password(conn.id)? else {
                return failed(
                    "A dedicated MCP database account is configured but its password is not stored.",
                );
            };
            conn.username = resolved.policy.mcp_username.clone();
            conn.password = password;
        }
        Ok(self.query_svc.run_readonly(&conn, request)?)
    }
}

struct Comparison {
    connections: Vec<ConnectionRecord>,
    servers: crate::models::ServersData,
    names: HashMap<i64, String>,
    discrepancies: Vec<Discrepancy>,
    hidden: usize,
}

/// Oracle names are stored upper-case; Postgres names keep the case the agent used.
fn actual_table_name(table: &str, db_type: DbType) -> String {
    match db_type {
        DbType::Oracle => table.to_ascii_uppercase(),
        DbType::Postgres => table.to_string(),
    }
}

/// Records on the audit target whether the result was cut short by the row/byte caps.
fn note_truncated(ctx: &mut Ctx, output: &Value) {
    if let (Some(target), Some(truncated)) = (ctx.target.as_mut(), output.get("truncated")) {
        target.insert("truncated".into(), truncated.clone());
    }
}

fn rows_to_json(result: ReadOnlyResult, limit: u32) -> Value {
    let mut bytes = 0usize;
    let mut rows: Vec<Value> = Vec::new();
    let mut truncated = result.truncated;
    for row in result.rows {
        bytes += row
            .iter()
            .map(|c| c.as_ref().map_or(4, |s| s.len() + 2))
            .sum::<usize>();
        if bytes > MAX_RESULT_BYTES {
            truncated = true;
            break;
        }
        rows.push(Value::Array(
            row.into_iter()
                .map(|c| c.map_or(Value::Null, Value::String))
                .collect(),
        ));
    }
    let mut out = Map::new();
    out.insert("columns".into(), json!(result.columns));
    out.insert("row_count".into(), json!(rows.len()));
    out.insert("rows".into(), Value::Array(rows));
    out.insert("truncated".into(), json!(truncated));
    out.insert("row_limit".into(), json!(limit));
    Value::Object(out)
}

#[cfg(test)]
#[path = "tests/mcp_gateway.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/mcp_gateway_targets.rs"]
mod target_tests;
