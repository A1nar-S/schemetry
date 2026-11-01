use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::anyhow;

use crate::models::{
    CompletionMetadata, ConnectionRecord, DbType, QueryServerResult, ScriptCompileError,
    ScriptServerResult,
};
use crate::repositories::db_repository::{
    ApplyDdlRequest, DbRepository, DdlStatement, LobCell, LobSource, ReadOnlyQuery, ReadOnlyResult,
};
use crate::services::cancel;
use crate::services::script_split::{ScriptStatement, split_script};

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

    pub fn completion_metadata(
        &self,
        conn: &ConnectionRecord,
    ) -> anyhow::Result<CompletionMetadata> {
        self.repo.fetch_completion_metadata(conn)
    }

    pub fn table_columns(
        &self,
        conn: &ConnectionRecord,
        table: &str,
    ) -> anyhow::Result<Vec<String>> {
        self.repo.fetch_table_columns(conn, table)
    }

    pub fn run_readonly(
        &self,
        conn: &ConnectionRecord,
        request: &ReadOnlyQuery,
    ) -> anyhow::Result<ReadOnlyResult> {
        self.repo.run_readonly_query(conn, request)
    }

    /// Runs `script` on every connection (its own account, no policy involved: this is the
    /// user's own action). Oracle runs statement by statement and reports compile errors per
    /// created object; Postgres runs the whole script in one transaction.
    pub fn apply_script_on_servers(
        &self,
        connections: &[ConnectionRecord],
        script: &str,
    ) -> Vec<ScriptServerResult> {
        let mut handles = Vec::new();
        for conn in connections.iter().cloned() {
            let repo = Arc::clone(&self.repo);
            let script = script.to_string();
            handles.push(cancel::spawn(move || {
                run_script(repo.as_ref(), &conn, &script)
            }));
        }
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
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
            handles.push(cancel::spawn(move || {
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
                            error: Some(if cancel::is_cancelled() {
                                cancel::CANCELLED.to_string()
                            } else {
                                err.to_string()
                            }),
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

const SCRIPT_TIMEOUT_SECS: u32 = 600;

fn run_script(
    repo: &dyn DbRepository,
    conn: &ConnectionRecord,
    script: &str,
) -> ScriptServerResult {
    let started = Instant::now();
    let statements = split_script(script, conn.db_type);
    let mut result = ScriptServerResult {
        server_id: conn.id,
        server_name: conn.name.clone(),
        statement_count: statements.len(),
        executed: 0,
        error: None,
        compile_errors: Vec::new(),
        duration_ms: 0,
    };
    let to_ddl = |s: &ScriptStatement| DdlStatement {
        sql: s.sql.clone(),
        object_type: s.object_type.clone(),
    };
    let describe = |index: usize, s: &ScriptStatement, error: String| {
        let head: String = s.sql.chars().take(80).collect();
        format!(
            "Statement {} ({}): {error}",
            index + 1,
            head.replace('\n', " ")
        )
    };

    if conn.db_type == DbType::Postgres {
        let request = ApplyDdlRequest {
            object_name: String::new(),
            statements: statements.iter().map(to_ddl).collect(),
            timeout_secs: SCRIPT_TIMEOUT_SECS,
        };
        match repo.apply_ddl(conn, &request) {
            Ok(_) => result.executed = statements.len(),
            Err(e) => result.error = Some(format!("{e} (rolled back; nothing was applied)")),
        }
    } else {
        for (index, statement) in statements.iter().enumerate() {
            if cancel::is_cancelled() {
                result.error = Some(cancel::CANCELLED.to_string());
                break;
            }
            let request = ApplyDdlRequest {
                object_name: statement.object_name.clone(),
                statements: vec![to_ddl(statement)],
                timeout_secs: SCRIPT_TIMEOUT_SECS,
            };
            match repo.apply_ddl(conn, &request) {
                Ok(done) => {
                    result.executed += 1;
                    result
                        .compile_errors
                        .extend(done.compile_errors.into_iter().map(|e| ScriptCompileError {
                            object: statement.object_name.clone(),
                            object_type: e.object_type,
                            severity: e.severity,
                            line: e.line,
                            position: e.position,
                            text: e.text,
                        }));
                }
                Err(e) => {
                    result.error = Some(describe(index, statement, e.to_string()));
                    break;
                }
            }
        }
    }
    result.duration_ms = started.elapsed().as_millis() as u64;
    result
}
