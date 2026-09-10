use std::sync::Arc;

use anyhow::Result;

use crate::models::{
    ConnectionRecord, DbType, HistoryFixResult, HistoryNamingRule, SchemaObject, TableDdls,
    TableFilterRule,
};
use crate::repositories::db_repository::{DbRepository, LobCell};
use crate::repositories::oracle_repository::DbOracleRepository;
use crate::repositories::postgres_repository::DbPostgresRepository;

/// Routes each call to the Oracle or Postgres repository based on
/// `ConnectionRecord::db_type`, so `AppState` and the services built on it only ever
/// depend on a single `Arc<dyn DbRepository>`.
pub struct DispatchRepository {
    oracle: Arc<DbOracleRepository>,
    postgres: Arc<DbPostgresRepository>,
}

impl DispatchRepository {
    pub fn new() -> Self {
        Self {
            oracle: Arc::new(DbOracleRepository::new()),
            postgres: Arc::new(DbPostgresRepository::new()),
        }
    }

    fn repo(&self, conn: &ConnectionRecord) -> &dyn DbRepository {
        match conn.db_type {
            DbType::Oracle => self.oracle.as_ref(),
            DbType::Postgres => self.postgres.as_ref(),
        }
    }
}

impl DbRepository for DispatchRepository {
    fn test_connection(&self, conn: &ConnectionRecord) -> Result<()> {
        self.repo(conn).test_connection(conn)
    }

    fn fetch_single(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<crate::models::ServerTables> {
        self.repo(conn).fetch_single(conn, filter_rules)
    }

    fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls> {
        self.repo(conn).fetch_table_ddls(conn, filter_rules)
    }

    fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls> {
        self.repo(conn).fetch_table_ddls_for_tables(conn, table_names)
    }

    fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>> {
        self.repo(conn).fetch_schema_objects(conn, filter_rules)
    }

    fn fetch_object_ddl(&self, conn: &ConnectionRecord, name: &str, object_type: &str) -> Result<String> {
        self.repo(conn).fetch_object_ddl(conn, name, object_type)
    }

    fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult> {
        self.repo(conn).generate_history_fix(conn, naming_rules)
    }

    fn run_query(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        materialize_lobs: bool,
    ) -> Result<(Vec<String>, Vec<String>, Vec<Vec<Option<String>>>)> {
        self.repo(conn).run_query(conn, sql, materialize_lobs)
    }

    fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        self.repo(conn).fetch_blob_cell(conn, sql, row_index, col_index, max_bytes)
    }

    fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> Result<LobCell> {
        self.repo(conn).fetch_lob_cell(conn, sql, row_index, col_index, max_bytes)
    }
}
