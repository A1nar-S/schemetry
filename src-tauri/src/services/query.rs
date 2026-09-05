use std::sync::Arc;
use std::thread;
use std::time::Instant;

use crate::models::{ConnectionRecord, QueryServerResult};
use crate::repositories::oracle_repository::{LobCell, OracleRepository};

pub struct QueryService {
    repo: Arc<dyn OracleRepository>,
}

impl QueryService {
    pub fn new(repo: Arc<dyn OracleRepository>) -> Self {
        Self { repo }
    }

    pub fn run_query_on_servers(
        &self,
        connections: &[ConnectionRecord],
        sql: &str,
        materialize_lobs: bool,
    ) -> Vec<QueryServerResult> {
        let sql_owned = sql.to_string();
        let mut handles = Vec::new();

        for conn in connections.iter().cloned() {
            let repo = Arc::clone(&self.repo);
            let sql_clone = sql_owned.clone();
            handles.push(thread::spawn(move || {
                let name = conn.name.clone();
                let started = Instant::now();
                match repo.run_query(&conn, &sql_clone, materialize_lobs) {
                    Ok((columns, column_types, rows)) => QueryServerResult {
                        server_name: name,
                        columns,
                        column_types,
                        rows,
                        error: None,
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                    Err(err) => QueryServerResult {
                        server_name: name,
                        columns: vec![],
                        column_types: vec![],
                        rows: vec![],
                        error: Some(err.to_string()),
                        duration_ms: started.elapsed().as_millis() as u64,
                    },
                }
            }));
        }

        let mut results: Vec<QueryServerResult> = handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect();
        results.sort_by(|a, b| a.server_name.cmp(&b.server_name));
        results
    }

    pub fn fetch_blob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> anyhow::Result<Vec<u8>> {
        self.repo
            .fetch_blob_cell(conn, sql, row_index, col_index, max_bytes)
    }

    pub fn fetch_lob_cell(
        &self,
        conn: &ConnectionRecord,
        sql: &str,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
    ) -> anyhow::Result<LobCell> {
        self.repo
            .fetch_lob_cell(conn, sql, row_index, col_index, max_bytes)
    }
}
