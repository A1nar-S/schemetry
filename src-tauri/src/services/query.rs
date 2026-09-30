use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use anyhow::anyhow;

use crate::models::{ConnectionRecord, QueryServerResult};
use crate::repositories::db_repository::{DbRepository, LobCell, LobSource};

type SharedLobSource = Arc<Mutex<Box<dyn LobSource>>>;

pub struct QueryService {
    repo: Arc<dyn DbRepository>,
    /// LOB sources of the latest run, keyed by `result_id`.
    lob_sources: Mutex<HashMap<u64, SharedLobSource>>,
    next_result_id: AtomicU64,
}

impl QueryService {
    pub fn new(repo: Arc<dyn DbRepository>) -> Self {
        Self {
            repo,
            lob_sources: Mutex::new(HashMap::new()),
            next_result_id: AtomicU64::new(1),
        }
    }

    pub fn run_query_on_servers(
        &self,
        connections: &[ConnectionRecord],
        sql: &str,
        materialize_lobs: bool,
    ) -> Vec<QueryServerResult> {
        // Release the previous run's sessions.
        self.lob_sources.lock().unwrap().clear();

        let sql_owned = sql.to_string();
        let mut handles = Vec::new();

        for conn in connections.iter().cloned() {
            let repo = Arc::clone(&self.repo);
            let sql_clone = sql_owned.clone();
            let result_id = self.next_result_id.fetch_add(1, Ordering::Relaxed);
            handles.push(thread::spawn(move || {
                let id = conn.id;
                let name = conn.name.clone();
                let started = Instant::now();
                match repo.run_query(&conn, &sql_clone, materialize_lobs) {
                    Ok(output) => (
                        QueryServerResult {
                            server_id: id,
                            server_name: name,
                            result_id,
                            columns: output.columns,
                            column_types: output.column_types,
                            rows: output.rows,
                            error: None,
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        output.lobs,
                    ),
                    Err(err) => (
                        QueryServerResult {
                            server_id: id,
                            server_name: name,
                            result_id,
                            columns: vec![],
                            column_types: vec![],
                            rows: vec![],
                            error: Some(err.to_string()),
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                        None,
                    ),
                }
            }));
        }

        let mut results = Vec::new();
        let mut sources = self.lob_sources.lock().unwrap();
        for (result, lobs) in handles.into_iter().filter_map(|h| h.join().ok()) {
            if let Some(lobs) = lobs {
                sources.insert(result.result_id, Arc::new(Mutex::new(lobs)));
            }
            results.push(result);
        }
        drop(sources);

        results.sort_by(|a, b| a.server_name.cmp(&b.server_name));
        results
    }

    fn lob_source(&self, result_id: u64) -> anyhow::Result<SharedLobSource> {
        self.lob_sources
            .lock()
            .unwrap()
            .get(&result_id)
            .cloned()
            .ok_or_else(|| anyhow!("This result is no longer available — re-run the query."))
    }

    /// Reads one LOB cell of a result: text capped at `max_chars`, bytes at `max_bytes`.
    pub fn read_lob_cell(
        &self,
        result_id: u64,
        row_index: usize,
        col_index: usize,
        max_bytes: usize,
        max_chars: usize,
    ) -> anyhow::Result<LobCell> {
        let source = self.lob_source(result_id)?;
        let mut source = source.lock().unwrap();
        source.read(row_index, col_index, max_bytes, max_chars)
    }

    /// Streams one LOB cell's full content into `out`, returning the byte count.
    pub fn copy_lob_cell(
        &self,
        result_id: u64,
        row_index: usize,
        col_index: usize,
        out: &mut dyn Write,
    ) -> anyhow::Result<u64> {
        let source = self.lob_source(result_id)?;
        let mut source = source.lock().unwrap();
        source.copy_to(row_index, col_index, out)
    }
}
