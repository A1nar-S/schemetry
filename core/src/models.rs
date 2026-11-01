use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Which database engine a connection targets. Determines which `DbRepository`
/// implementation handles it (see `repositories::dispatch_repository`) and how a few
/// fields on `ConnectionRecord` are interpreted (`service_name`, `pg_schema`).
/// `#[serde(default)]` makes existing rows/exported JSON without this field load as
/// `Oracle`, since that was the only engine before Postgres support was added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    #[default]
    Oracle,
    Postgres,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRecord {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub db_type: DbType,
    pub host: String,
    pub port: u16,
    /// Oracle: the service name (e.g. `ORCL`). Postgres: the database name.
    pub service_name: String,
    pub username: String,
    pub password: String,
    pub group_name: String,
    /// Postgres only: the schema to introspect/target within `service_name`'s
    /// database. Empty means `public`. Unused for Oracle connections, where the
    /// schema is always the connecting `username`.
    #[serde(default)]
    pub pg_schema: String,
    /// True when the password could not be loaded from the OS credential manager
    /// (e.g. the keyring entry was deleted or reset outside the app). `password` is
    /// empty in that case; the row is still shown so the user can fix or delete it.
    #[serde(default)]
    pub password_broken: bool,
}

/// Overrides the default `<schema_root_folder>/<schema>` location, the default DDL file
/// encoding, the default per-object-type DDL file extensions and storage modes, the
/// default file naming convention, the default code/migration subfolder names, and/or the
/// default migration folder mode/label for a single schema, for schemas whose folder
/// doesn't live under the standard root (e.g. a differently-named folder, or a folder
/// checked out elsewhere entirely), whose files need a different encoding than the
/// global default, or that need different file extensions, storage modes, naming, or
/// folder layout than the global default. Any field may be left empty to fall back to the
/// global setting. `extensions` and `storage_modes` are each a JSON object mapping
/// upper-cased object type (e.g. `"TABLE"`) to a value; a type missing from the map falls
/// back to the global setting for that type. `naming_convention` is a single value
/// (`"timestamp"` or `"flyway"`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFolderOverride {
    #[serde(default)]
    pub id: i64,
    pub schema_name: String,
    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub encoding: String,
    #[serde(default)]
    pub extensions: String,
    #[serde(default)]
    pub storage_modes: String,
    #[serde(default)]
    pub naming_convention: String,
    #[serde(default)]
    pub code_folder_name: String,
    #[serde(default)]
    pub migration_folder_name: String,
    #[serde(default)]
    pub migration_folder_mode: String,
    #[serde(default)]
    pub migration_version_label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub column_name: Option<String>,
    pub data_type: Option<String>,
    pub data_length: Option<String>,
    pub data_default: Option<String>,
    pub comments: Option<String>,
    pub index_name: Option<String>,
    pub index_position: Option<u32>,
}

pub type TableColumns = HashMap<String, ColumnInfo>;
pub type ServerTables = HashMap<String, TableColumns>;
/// Keyed by connection `id`, not name — names aren't guaranteed unique across
/// groups/engines, but a connection's numeric id always is.
pub type ServersData = HashMap<i64, ServerTables>;
pub type TableDdls = HashMap<String, String>;
pub type ServerTableDdls = HashMap<i64, TableDdls>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QueryServerResult {
    /// Identity of the connection this result came from — use this, not `server_name`,
    /// to correlate a result back to its connection (names may repeat).
    pub server_id: i64,
    pub server_name: String,
    /// Key for this result's LOB cells.
    #[serde(default)]
    pub result_id: u64,
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueryHistoryEntry {
    pub id: i64,
    pub sql_text: String,
    pub pinned: bool,
    pub favorite: bool,
    pub description: String,
    /// UTC, as SQLite's `CURRENT_TIMESTAMP` writes it: `YYYY-MM-DD HH:MM:SS`.
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discrepancy {
    pub difference: String,
    pub element: String,
    pub table_name: String,
    pub column_name: String,
    /// Identity of the connection this discrepancy was found on — use this, not
    /// `server_name`, for any comparison/dispatch logic (names may repeat).
    pub server_id: i64,
    pub server_name: String,
    pub details: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaObject {
    pub name: String,
    pub object_type: String,
}

/// Schema metadata for SQL autocompletion in the query editor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionMetadata {
    /// The schema the names belong to (Oracle user / Postgres schema).
    pub schema: String,
    /// Tables, views and materialized views, with their columns in order.
    pub relations: Vec<CompletionRelation>,
    /// Other objects completed by name: sequences, functions, procedures, packages, …
    pub objects: Vec<SchemaObject>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionRelation {
    pub name: String,
    /// `TABLE`, `VIEW` or `MATERIALIZED VIEW`.
    pub kind: String,
    pub columns: Vec<CompletionColumn>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompletionColumn {
    pub name: String,
    pub data_type: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryTableIssue {
    pub history_table: String,
    pub column_name: String,
    pub issue_type: String, // "MISSING" or "TYPE_MISMATCH"
    pub main_type: String,
    pub history_type: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HistoryFixResult {
    pub issues: Vec<HistoryTableIssue>,
    pub fix_sql: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ServerHistoryFixResult {
    pub server_id: i64,
    pub server_name: String,
    pub issues: Vec<HistoryTableIssue>,
    pub fix_sql: String,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilterAction {
    Exclude,
    Include,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchType {
    Prefix,
    Suffix,
    Contains,
    Exact,
}

/// A user-defined rule for including/excluding tables (and, where applied, other
/// schema objects) by name pattern, replacing what used to be hardcoded naming
/// conventions in the Oracle queries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableFilterRule {
    #[serde(default)]
    pub id: i64,
    pub action: FilterAction,
    pub match_type: MatchType,
    pub pattern: String,
    pub enabled: bool,
}

/// A user-defined rule for pairing a main table with its history-table counterpart by
/// name pattern (e.g. a `HIST_` prefix or `_HIST` suffix). Multiple rules may be
/// enabled at once so several naming conventions can coexist. Only `Prefix` and
/// `Suffix` make sense here (they're used to derive the counterpart name); `Contains`
/// and `Exact` are rejected by the repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryNamingRule {
    #[serde(default)]
    pub id: i64,
    pub match_type: MatchType,
    pub pattern: String,
    pub enabled: bool,
}

/// Whether the MCP server may read table data through a connection at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpDataAccess {
    /// No table data, ever.
    #[default]
    None,
    /// Only tables/columns matching `McpPolicy::table_rules`.
    Allowlist,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpRuleAction {
    Allow,
    Deny,
}

/// One data-access rule. `table` and `column` accept `*` / `?` wildcards; an empty
/// `column` means every column of the matching tables. Deny always wins over allow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTableRule {
    pub action: McpRuleAction,
    pub table: String,
    #[serde(default)]
    pub column: String,
}

/// Hides matching objects from MCP schema listings, DDL reads and compare output.
/// `object_type` is `*` or an object type such as `TABLE`; `name` accepts wildcards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpObjectDenyRule {
    #[serde(default = "McpObjectDenyRule::any_type")]
    pub object_type: String,
    pub name: String,
}

impl McpObjectDenyRule {
    fn any_type() -> String {
        "*".to_string()
    }
}

/// What an agent may do through the MCP server on one connection. The defaults are the
/// most restrictive useful level and also apply to connections without a stored policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpPolicy {
    pub connection_id: i64,
    /// Connection visible to MCP at all (explicit opt-in).
    pub mcp_enabled: bool,
    pub allow_schema_list: bool,
    pub allow_ddl_read: bool,
    pub allow_save_scripts: bool,
    pub allow_compare_fix: bool,
    pub data_access: McpDataAccess,
    /// Free-form SQL (`run_query`); still needs `data_access = allowlist`.
    pub allow_raw_sql: bool,
    pub table_rules: Vec<McpTableRule>,
    pub object_deny: Vec<McpObjectDenyRule>,
    pub max_rows: u32,
    pub query_timeout_secs: u32,
    /// Optional dedicated database account used for table-data reads only. Its
    /// password lives in the OS keychain and is never part of this struct.
    pub mcp_username: String,
    /// Output only: whether a password is stored for `mcp_username`.
    pub mcp_password_set: bool,
}

impl McpPolicy {
    pub const DEFAULT_MAX_ROWS: u32 = 200;
    pub const DEFAULT_TIMEOUT_SECS: u32 = 30;
    pub const MAX_ROWS_CEILING: u32 = 10_000;
    pub const TIMEOUT_CEILING_SECS: u32 = 600;
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            connection_id: 0,
            mcp_enabled: false,
            allow_schema_list: true,
            allow_ddl_read: true,
            allow_save_scripts: false,
            allow_compare_fix: false,
            data_access: McpDataAccess::None,
            allow_raw_sql: false,
            table_rules: Vec::new(),
            object_deny: Vec::new(),
            max_rows: Self::DEFAULT_MAX_ROWS,
            query_timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
            mcp_username: String::new(),
            mcp_password_set: false,
        }
    }
}

/// How an audit entry ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpOutcome {
    Allowed,
    Denied,
    Error,
}

impl McpOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            McpOutcome::Allowed => "allowed",
            McpOutcome::Denied => "denied",
            McpOutcome::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allowed" => Some(McpOutcome::Allowed),
            "denied" => Some(McpOutcome::Denied),
            "error" => Some(McpOutcome::Error),
            _ => None,
        }
    }
}

/// One row of the append-only MCP audit log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAuditEntry {
    pub id: i64,
    /// UTC, RFC 3339 with milliseconds.
    pub timestamp: String,
    pub tool: String,
    pub connection_id: Option<i64>,
    pub connection_name: String,
    /// JSON text of the call arguments (credentials never included).
    pub arguments: String,
    pub outcome: McpOutcome,
    /// Why a call was denied or what error occurred.
    pub reason: String,
    pub row_count: Option<i64>,
    pub bytes: i64,
    pub duration_ms: i64,
    pub client_name: String,
    pub prev_hash: String,
    pub hash: String,
}

/// Filters for browsing the audit log; every field is optional.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpAuditFilter {
    pub connection_id: Option<i64>,
    pub tool: Option<String>,
    pub outcome: Option<McpOutcome>,
    /// Case-insensitive text matched against the entry's target names (tables, objects).
    pub target: Option<String>,
    /// Inclusive lower bound, `YYYY-MM-DD` or a full RFC 3339 timestamp (UTC).
    pub from: Option<String>,
    /// Inclusive upper bound; a bare date covers that whole day.
    pub to: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Result of walking the audit hash chain.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAuditVerification {
    pub ok: bool,
    pub entries: i64,
    /// Human-readable description of the first problem found.
    pub problem: Option<String>,
    pub first_bad_id: Option<i64>,
}
