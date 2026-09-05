use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRecord {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub service_name: String,
    pub username: String,
    pub password: String,
    pub group_name: String,
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
pub type ServersData = HashMap<String, ServerTables>;
pub type TableDdls = HashMap<String, String>;
pub type ServerTableDdls = HashMap<String, TableDdls>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QueryServerResult {
    pub server_name: String,
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discrepancy {
    pub difference: String,
    pub element: String,
    pub table_name: String,
    pub column_name: String,
    pub server_name: String,
    pub details: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaObject {
    pub name: String,
    pub object_type: String,
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
