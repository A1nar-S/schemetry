use anyhow::Result;

use crate::models::{
    ConnectionRecord, HistoryFixResult, HistoryNamingRule, SchemaObject, TableDdls,
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
    /// Returns `(column names, column type labels, rows)`. When `materialize_lobs` is
    /// false, binary LOB cells render as `<BLOB>` and text LOB cells as `<CLOB>` (their
    /// content can be fetched lazily via [`DbRepository::fetch_lob_cell`]). When
    /// true, LOB content is materialized inline (capped): CLOB → text, BLOB → decoded
    /// text or hex.
    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Vec<Option<String>>>)>;
    /// Re-run `sql` and read the raw bytes of a single BLOB/binary cell, capped at
    /// `max_bytes`. Used for the full-bytes "Save to file" path.
    fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<Vec<u8>>;
    /// Re-run `sql` and read a single LOB cell, returning text for CLOB-like columns
    /// and bytes (capped at `max_bytes`) for binary columns, based on the column type.
    fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<LobCell>;
}

/// A single LOB cell fetched on demand for the content viewer.
pub enum LobCell {
    Text(Option<String>),
    Binary(Vec<u8>),
}
