use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params_from_iter};

use crate::models::{McpAuditEntry, McpAuditFilter, McpOutcome};

/// Reads and appends only. There is deliberately no update/delete/purge method here, and
/// triggers make SQLite itself reject both, so nothing in Schemetry can clean the log.
pub trait McpAuditRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    /// Appends one entry. `build` receives the next id and the previous entry's hash
    /// (all zeros for the first entry) inside a write transaction, so concurrent MCP
    /// processes always extend a single chain.
    fn append_chained(
        &self,
        build: &mut dyn FnMut(i64, &str) -> McpAuditEntry,
    ) -> Result<McpAuditEntry>;
    /// Newest first.
    fn list(&self, filter: &McpAuditFilter) -> Result<Vec<McpAuditEntry>>;
    fn count(&self, filter: &McpAuditFilter) -> Result<i64>;
    /// Visits every entry oldest first until `visit` returns `false`.
    fn for_each_in_order(&self, visit: &mut dyn FnMut(McpAuditEntry) -> bool) -> Result<()>;
    /// The highest id ever handed out (SQLite's AUTOINCREMENT counter); a value above the
    /// newest stored id means trailing rows were removed.
    fn issued_sequence(&self) -> Result<i64>;
}

pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub struct SqliteMcpAuditRepository {
    /// Overrides the app-wide database path (used by tests for isolated files).
    path: Option<std::path::PathBuf>,
}

impl SqliteMcpAuditRepository {
    pub fn new() -> Self {
        Self { path: None }
    }

    pub fn with_path(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn open(&self) -> Result<Connection> {
        let path = self
            .path
            .clone()
            .unwrap_or_else(crate::utils::paths::db_path);
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        Ok(conn)
    }
}

impl Default for SqliteMcpAuditRepository {
    fn default() -> Self {
        Self::new()
    }
}

const COLUMNS: &str = "id, timestamp, tool, connection_id, connection_name, arguments, outcome, \
    reason, row_count, bytes, duration_ms, client_name, prev_hash, hash";

fn row_to_entry(row: &rusqlite::Row) -> rusqlite::Result<McpAuditEntry> {
    let outcome: String = row.get(6)?;
    Ok(McpAuditEntry {
        id: row.get(0)?,
        timestamp: row.get(1)?,
        tool: row.get(2)?,
        connection_id: row.get(3)?,
        connection_name: row.get(4)?,
        arguments: row.get(5)?,
        // An unknown value can only come from outside tampering; surface it as an error entry.
        outcome: McpOutcome::parse(&outcome).unwrap_or(McpOutcome::Error),
        reason: row.get(7)?,
        row_count: row.get(8)?,
        bytes: row.get(9)?,
        duration_ms: row.get(10)?,
        client_name: row.get(11)?,
        prev_hash: row.get(12)?,
        hash: row.get(13)?,
    })
}

fn where_clause(filter: &McpAuditFilter) -> (String, Vec<rusqlite::types::Value>) {
    use rusqlite::types::Value;

    let mut clauses: Vec<&str> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if let Some(id) = filter.connection_id {
        clauses.push("connection_id = ?");
        values.push(Value::Integer(id));
    }
    if let Some(tool) = filter.tool.as_deref().filter(|t| !t.trim().is_empty()) {
        clauses.push("tool = ?");
        values.push(Value::Text(tool.trim().to_string()));
    }
    if let Some(outcome) = filter.outcome {
        clauses.push("outcome = ?");
        values.push(Value::Text(outcome.as_str().to_string()));
    }
    if let Some(target) = filter
        .target
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        // json_valid guards entries whose arguments are not JSON (tampered or foreign).
        clauses.push(
            "(CASE WHEN json_valid(arguments) THEN json_extract(arguments, '$.target.names') END) \
             LIKE ? ESCAPE '\\'",
        );
        let escaped = target
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.push(Value::Text(format!("%{escaped}%")));
    }
    if let Some(from) = filter.from.as_deref().filter(|t| !t.trim().is_empty()) {
        clauses.push("timestamp >= ?");
        values.push(Value::Text(from.trim().to_string()));
    }
    if let Some(to) = filter
        .to
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        clauses.push("timestamp <= ?");
        // A bare date means "through the end of that day".
        values.push(Value::Text(if to.len() == 10 {
            format!("{to}T23:59:59.999Z")
        } else {
            to.to_string()
        }));
    }

    if clauses.is_empty() {
        (String::new(), values)
    } else {
        (format!(" WHERE {}", clauses.join(" AND ")), values)
    }
}

impl McpAuditRepository for SqliteMcpAuditRepository {
    fn init_db(&self) -> Result<()> {
        let conn = self.open()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_audit_log (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp       TEXT    NOT NULL,
                tool            TEXT    NOT NULL,
                connection_id   INTEGER,
                connection_name TEXT    NOT NULL DEFAULT '',
                arguments       TEXT    NOT NULL DEFAULT '',
                outcome         TEXT    NOT NULL,
                reason          TEXT    NOT NULL DEFAULT '',
                row_count       INTEGER,
                bytes           INTEGER NOT NULL DEFAULT 0,
                duration_ms     INTEGER NOT NULL DEFAULT 0,
                client_name     TEXT    NOT NULL DEFAULT '',
                prev_hash       TEXT    NOT NULL,
                hash            TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_mcp_audit_timestamp ON mcp_audit_log(timestamp);
            CREATE TRIGGER IF NOT EXISTS mcp_audit_log_no_update
                BEFORE UPDATE ON mcp_audit_log
                BEGIN SELECT RAISE(ABORT, 'mcp_audit_log is append-only'); END;
            CREATE TRIGGER IF NOT EXISTS mcp_audit_log_no_delete
                BEFORE DELETE ON mcp_audit_log
                BEGIN SELECT RAISE(ABORT, 'mcp_audit_log is append-only'); END;",
        )
        .context("Failed to initialize the MCP audit log")?;
        Ok(())
    }

    fn append_chained(
        &self,
        build: &mut dyn FnMut(i64, &str) -> McpAuditEntry,
    ) -> Result<McpAuditEntry> {
        let mut conn = self.open()?;
        // IMMEDIATE takes the write lock up front so two processes can't read the same tail.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

        let last: Option<(i64, String)> = tx
            .query_row(
                "SELECT id, hash FROM mcp_audit_log ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let issued: i64 = tx
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'mcp_audit_log'",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let (last_id, prev_hash) = last.unwrap_or((0, GENESIS_HASH.to_string()));
        let next_id = last_id.max(issued) + 1;

        let entry = build(next_id, &prev_hash);
        tx.execute(
            "INSERT INTO mcp_audit_log (id, timestamp, tool, connection_id, connection_name,
                arguments, outcome, reason, row_count, bytes, duration_ms, client_name,
                prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                entry.id,
                entry.timestamp,
                entry.tool,
                entry.connection_id,
                entry.connection_name,
                entry.arguments,
                entry.outcome.as_str(),
                entry.reason,
                entry.row_count,
                entry.bytes,
                entry.duration_ms,
                entry.client_name,
                entry.prev_hash,
                entry.hash,
            ],
        )
        .context("Failed to append to the MCP audit log")?;
        tx.commit()?;
        Ok(entry)
    }

    fn list(&self, filter: &McpAuditFilter) -> Result<Vec<McpAuditEntry>> {
        let conn = self.open()?;
        let (clause, values) = where_clause(filter);
        let limit = i64::from(filter.limit.unwrap_or(500).clamp(1, 10_000));
        let offset = i64::from(filter.offset.unwrap_or(0));
        let sql = format!(
            "SELECT {COLUMNS} FROM mcp_audit_log{clause} ORDER BY id DESC LIMIT {limit} OFFSET {offset}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map(params_from_iter(values), row_to_entry)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn count(&self, filter: &McpAuditFilter) -> Result<i64> {
        let conn = self.open()?;
        let (clause, values) = where_clause(filter);
        let count = conn.query_row(
            &format!("SELECT COUNT(1) FROM mcp_audit_log{clause}"),
            params_from_iter(values),
            |row| row.get(0),
        )?;
        Ok(count)
    }

    fn for_each_in_order(&self, visit: &mut dyn FnMut(McpAuditEntry) -> bool) -> Result<()> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM mcp_audit_log ORDER BY id"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if !visit(row_to_entry(row)?) {
                break;
            }
        }
        Ok(())
    }

    fn issued_sequence(&self) -> Result<i64> {
        let conn = self.open()?;
        let issued = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'mcp_audit_log'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(issued.unwrap_or(0))
    }
}
