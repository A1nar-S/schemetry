use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::models::{
    ConnectionRecord, HistoryFixResult, HistoryNamingRule, InvalidObjectList, ObjectErrorList,
    SchemaObject, ServersData, TableDdls, TableFilterRule,
};
use crate::repositories::db_repository::{ApplyDdlRequest, ApplyDdlResult, DbRepository};
use crate::repositories::oracle_repository::configure_client_lib_dir;
use crate::services::cancel;

pub struct SchemaDiffService {
    repo: Arc<dyn DbRepository>,
}

impl SchemaDiffService {
    pub fn new(repo: Arc<dyn DbRepository>) -> Self {
        Self { repo }
    }

    pub fn test_connection(&self, conn: &ConnectionRecord) -> Result<()> {
        self.repo.test_connection(conn)
    }

    pub fn fetch_from_connections(
        &self,
        connections: &[ConnectionRecord],
        filter_rules: &[TableFilterRule],
    ) -> (ServersData, HashMap<i64, String>) {
        let mut handles = Vec::new();
        for conn in connections.iter().cloned() {
            let repo = Arc::clone(&self.repo);
            let rules = filter_rules.to_vec();
            handles.push(cancel::spawn(move || {
                let id = conn.id;
                match repo.fetch_single(&conn, &rules) {
                    Ok(tables) => (id, Some(tables), None),
                    Err(err) => (id, None, Some(err.to_string())),
                }
            }));
        }

        let mut servers: ServersData = HashMap::new();
        let mut errors: HashMap<i64, String> = HashMap::new();
        for handle in handles {
            let join_result = handle.join();
            if let Ok((id, tables_opt, err_opt)) = join_result {
                if let Some(tables) = tables_opt {
                    servers.insert(id, tables);
                }
                if let Some(err) = err_opt {
                    errors.insert(id, err);
                }
            }
        }
        (servers, errors)
    }

    pub fn fetch_table_ddls(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<TableDdls> {
        self.repo.fetch_table_ddls(conn, filter_rules)
    }

    pub fn fetch_table_ddls_for_tables(
        &self,
        conn: &ConnectionRecord,
        table_names: &[String],
    ) -> Result<TableDdls> {
        self.repo.fetch_table_ddls_for_tables(conn, table_names)
    }

    pub fn fetch_schema_objects(
        &self,
        conn: &ConnectionRecord,
        filter_rules: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>> {
        self.repo.fetch_schema_objects(conn, filter_rules)
    }

    pub fn fetch_object_ddl(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
    ) -> Result<String> {
        self.repo.fetch_object_ddl(conn, name, object_type)
    }

    pub fn fetch_invalid_objects(
        &self,
        conn: &ConnectionRecord,
        object_type: Option<&str>,
        max_rows: usize,
    ) -> Result<InvalidObjectList> {
        self.repo.fetch_invalid_objects(conn, object_type, max_rows)
    }

    pub fn fetch_object_errors(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
        max_rows: usize,
    ) -> Result<ObjectErrorList> {
        self.repo
            .fetch_object_errors(conn, name, object_type, max_rows)
    }

    pub fn apply_ddl(
        &self,
        conn: &ConnectionRecord,
        request: &ApplyDdlRequest,
    ) -> Result<ApplyDdlResult> {
        self.repo.apply_ddl(conn, request)
    }

    pub fn generate_history_fix(
        &self,
        conn: &ConnectionRecord,
        naming_rules: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult> {
        self.repo.generate_history_fix(conn, naming_rules)
    }

    /// Initialise the Oracle Instant Client library directory.
    pub fn configure_client_lib_dir(&self, dir: &str) -> Result<bool> {
        configure_client_lib_dir(dir)
    }
}
