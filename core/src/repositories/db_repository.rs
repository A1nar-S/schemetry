use std::io::Write;

use anyhow::Result;

use crate::models::{
    CompletionMetadata, ConnectionRecord, HistoryFixResult, HistoryNamingRule, InvalidObjectList,
    ObjectErrorList, SchemaObject, TableDdls, TableFilterRule,
};

/// Engine-agnostic schema/query interface. `DbOracleRepository` and
/// `DbPostgresRepository` both implement this; `DispatchRepository` picks between
/// them per-connection based on `ConnectionRecord::db_type`, so `AppState` and the
/// services built on top only ever depend on this trait, never on a concrete engine.
pub trait DbRepository: Send + Sync {
    fn test_connection(&self, conn: &ConnectionRecord) -> Result<()>;
    fn fetch_single(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<crate::models::ServerTables>;
    fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls>;
    fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls>;
    fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>>;
    fn fetch_object_ddl(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
    ) -> Result<String>;
    /// Pair up main tables with their history-table counterpart using the given
    /// enabled naming rules (e.g. a `HIST_` prefix and/or a `_HIST` suffix rule) and
    /// report any column drift between them. Only `Prefix`/`Suffix` rules are honored;
    /// other match types are ignored. An empty rule list yields an empty result.
    fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult>;
    /// LOB cells show as `<BLOB>`/`<CLOB>`, or as capped inline content when
    /// `materialize_lobs`; either way they stay readable via [`QueryOutput::lobs`].
    /// Tables/views with their columns, plus other named objects, for editor
    /// autocompletion. Ignores the compare filter rules.
    fn fetch_completion_metadata(&self, conn: &ConnectionRecord) -> Result<CompletionMetadata>;
    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<QueryOutput>;
    /// Column names of one table of the connection's schema, in table order (empty when
    /// the table doesn't exist or isn't visible).
    fn fetch_table_columns(&self, conn: &ConnectionRecord, table: &str) -> Result<Vec<String>>;
    /// Runs one SELECT inside a read-only transaction with a row cap and a timeout —
    /// the execution path behind the MCP data tools.
    fn run_readonly_query(
        &self,
        conn: &ConnectionRecord,
        request: &ReadOnlyQuery,
    ) -> Result<ReadOnlyResult>;
    /// Objects of the connection's schema the database flags as invalid (Oracle), at most
    /// `max_rows`, optionally of one `object_type`. Engines without the concept return a
    /// "not applicable" result rather than an error.
    fn fetch_invalid_objects(
        &self,
        conn: &ConnectionRecord,
        object_type: Option<&str>,
        max_rows: usize,
    ) -> Result<InvalidObjectList>;
    /// Compile errors and warnings of one object of the connection's schema, at most
    /// `max_rows`; see [`DbRepository::fetch_invalid_objects`] for engines without them.
    fn fetch_object_errors(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
        max_rows: usize,
    ) -> Result<ObjectErrorList>;
    /// Executes already-validated `CREATE` statements in the connection's own schema —
    /// the execution path behind the MCP `apply_ddl` tool. Oracle also reports the
    /// object's compile errors.
    fn apply_ddl(
        &self,
        conn: &ConnectionRecord,
        request: &ApplyDdlRequest,
    ) -> Result<ApplyDdlResult>;
}

/// One validated statement, ready to send to the database as is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DdlStatement {
    pub sql: String,
    /// Upper-case, e.g. `PACKAGE BODY`.
    pub object_type: String,
}

pub struct ApplyDdlRequest {
    /// Normalised object name (upper-case for Oracle).
    pub object_name: String,
    pub statements: Vec<DdlStatement>,
    pub timeout_secs: u32,
}

/// An `ALL_ERRORS` row left behind by compiling a stored program unit.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DdlCompileError {
    pub object_type: String,
    /// `ERROR` or `WARNING`.
    pub severity: String,
    pub line: i64,
    pub position: i64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyDdlResult {
    pub compile_errors: Vec<DdlCompileError>,
}

/// A bounded, read-only query. `binds` are Oracle `:1`, `:2`, … values; Postgres queries
/// carry their (escaped) literals inline and pass none.
pub struct ReadOnlyQuery {
    pub sql: String,
    pub binds: Vec<String>,
    pub max_rows: u32,
    pub timeout_secs: u32,
}

pub struct ReadOnlyResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// More rows existed than `max_rows`; the extras were dropped.
    pub truncated: bool,
}

pub struct QueryOutput {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    /// `None` when the result has no LOB columns.
    pub lobs: Option<Box<dyn LobSource>>,
}

/// Keeps a result's LOB cells readable after `run_query`, so nothing is re-queried.
pub trait LobSource: Send {
    /// Reads one cell, capped at `max_chars` (text) or `max_bytes` (binary).
    fn read(
        &mut self,
        row: usize,
        col: usize,
        max_bytes: usize,
        max_chars: usize,
    ) -> Result<LobCell>;
    /// Streams a binary cell's full content into `out`, returning the byte count.
    fn copy_to(&mut self, row: usize, col: usize, out: &mut dyn Write) -> Result<u64>;
}

/// A single LOB cell fetched on demand for the content viewer.
#[derive(Debug, PartialEq)]
pub enum LobCell {
    Text(Option<String>),
    Binary(Vec<u8>),
}
