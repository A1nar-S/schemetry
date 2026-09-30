use std::io::Write;

use anyhow::Result;

use crate::models::{
    CompletionMetadata, ConnectionRecord, HistoryFixResult, HistoryNamingRule, SchemaObject, TableDdls,
    TableFilterRule,
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
    fn fetch_object_ddl(&self, conn: &ConnectionRecord, name: &str, object_type: &str) -> Result<String>;
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
    fn read(&mut self, row: usize, col: usize, max_bytes: usize, max_chars: usize) -> Result<LobCell>;
    /// Streams a binary cell's full content into `out`, returning the byte count.
    fn copy_to(&mut self, row: usize, col: usize, out: &mut dyn Write) -> Result<u64>;
}

/// A single LOB cell fetched on demand for the content viewer.
#[derive(Debug, PartialEq)]
pub enum LobCell {
    Text(Option<String>),
    Binary(Vec<u8>),
}
