use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Where a deploy request stands. Only `Pending` → `Approved`/`Rejected` happens in the
/// GUI; `Approved` → `Consumed` happens once, when the agent's retry executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApplyStatus {
    Pending,
    Approved,
    Rejected,
    Consumed,
}

impl ApplyStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ApplyStatus::Pending => "pending",
            ApplyStatus::Approved => "approved",
            ApplyStatus::Rejected => "rejected",
            ApplyStatus::Consumed => "consumed",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "approved" => ApplyStatus::Approved,
            "rejected" => ApplyStatus::Rejected,
            "consumed" => ApplyStatus::Consumed,
            _ => ApplyStatus::Pending,
        }
    }
}

/// What an agent asked to deploy. `sha256` is the digest of `ddl`; an approval only ever
/// matches a request with the same connection, object, type and digest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewApplyRequest {
    pub connection_id: i64,
    pub object_type: String,
    pub object_name: String,
    pub sha256: String,
    pub bytes: i64,
    /// Human-readable plan, e.g. `CREATE OR REPLACE PACKAGE + PACKAGE BODY`.
    pub summary: String,
    /// Kept only so the user can review it before approving; cleared once consumed.
    pub ddl: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyRequest {
    pub id: i64,
    pub connection_id: i64,
    pub object_type: String,
    pub object_name: String,
    pub sha256: String,
    pub bytes: i64,
    pub summary: String,
    pub ddl: String,
    pub status: ApplyStatus,
    /// Unix seconds.
    pub created_at: i64,
    /// Unix seconds; a pending request lapses here, an approval stops being usable here.
    pub expires_at: i64,
}

pub trait McpApprovalRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    /// Records a pending request, or returns the live (pending/approved, unexpired) one
    /// for the same connection/object/digest so retries don't pile up.
    fn request(&self, new: &NewApplyRequest, now: i64, pending_until: i64) -> Result<ApplyRequest>;
    /// Atomically turns a live approval for exactly this connection/object/digest into
    /// `Consumed` and returns its id; `None` when there is none.
    fn consume(
        &self,
        connection_id: i64,
        object_type: &str,
        object_name: &str,
        sha256: &str,
        now: i64,
    ) -> Result<Option<i64>>;
    /// Approves or rejects a live pending request. `false` when it no longer is one.
    fn decide(&self, id: i64, approve: bool, now: i64, approved_until: i64) -> Result<bool>;
    /// Pending and approved requests that have not expired, newest first.
    fn list_open(&self, now: i64) -> Result<Vec<ApplyRequest>>;
    /// Drops rows that expired before `before`.
    fn prune(&self, before: i64) -> Result<()>;
}

pub struct SqliteMcpApprovalRepository {
    /// Overrides the app-wide database path (used by tests for isolated files).
    path: Option<PathBuf>,
}

impl SqliteMcpApprovalRepository {
    pub fn new() -> Self {
        Self { path: None }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn open(&self) -> Result<Connection> {
        let path = self
            .path
            .clone()
            .unwrap_or_else(crate::utils::paths::db_path);
        Ok(Connection::open(path)?)
    }
}

impl Default for SqliteMcpApprovalRepository {
    fn default() -> Self {
        Self::new()
    }
}

const COLUMNS: &str = "id, connection_id, object_type, object_name, sha256, bytes, summary, ddl, \
    status, created_at, expires_at";

fn row_to_request(row: &rusqlite::Row) -> rusqlite::Result<ApplyRequest> {
    let status: String = row.get(8)?;
    Ok(ApplyRequest {
        id: row.get(0)?,
        connection_id: row.get(1)?,
        object_type: row.get(2)?,
        object_name: row.get(3)?,
        sha256: row.get(4)?,
        bytes: row.get(5)?,
        summary: row.get(6)?,
        ddl: row.get(7)?,
        status: ApplyStatus::parse(&status),
        created_at: row.get(9)?,
        expires_at: row.get(10)?,
    })
}

impl McpApprovalRepository for SqliteMcpApprovalRepository {
    fn init_db(&self) -> Result<()> {
        self.open()?.execute(
            "CREATE TABLE IF NOT EXISTS mcp_apply_approvals (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                connection_id INTEGER NOT NULL,
                object_type   TEXT    NOT NULL,
                object_name   TEXT    NOT NULL,
                sha256        TEXT    NOT NULL,
                bytes         INTEGER NOT NULL,
                summary       TEXT    NOT NULL,
                ddl           TEXT    NOT NULL,
                status        TEXT    NOT NULL DEFAULT 'pending',
                created_at    INTEGER NOT NULL,
                expires_at    INTEGER NOT NULL,
                decided_at    INTEGER,
                consumed_at   INTEGER
            )",
            [],
        )?;
        Ok(())
    }

    fn request(&self, new: &NewApplyRequest, now: i64, pending_until: i64) -> Result<ApplyRequest> {
        let conn = self.open()?;
        let existing = conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM mcp_apply_approvals
                     WHERE connection_id = ?1 AND object_type = ?2 AND object_name = ?3
                       AND sha256 = ?4 AND status IN ('pending', 'approved') AND expires_at > ?5
                     ORDER BY id DESC LIMIT 1"
                ),
                params![
                    new.connection_id,
                    new.object_type,
                    new.object_name,
                    new.sha256,
                    now
                ],
                row_to_request,
            )
            .optional()?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        conn.execute(
            "INSERT INTO mcp_apply_approvals (
                connection_id, object_type, object_name, sha256, bytes, summary, ddl,
                status, created_at, expires_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?9)",
            params![
                new.connection_id,
                new.object_type,
                new.object_name,
                new.sha256,
                new.bytes,
                new.summary,
                new.ddl,
                now,
                pending_until
            ],
        )
        .context("Failed to record the deploy request")?;
        Ok(ApplyRequest {
            id: conn.last_insert_rowid(),
            connection_id: new.connection_id,
            object_type: new.object_type.clone(),
            object_name: new.object_name.clone(),
            sha256: new.sha256.clone(),
            bytes: new.bytes,
            summary: new.summary.clone(),
            ddl: new.ddl.clone(),
            status: ApplyStatus::Pending,
            created_at: now,
            expires_at: pending_until,
        })
    }

    fn consume(
        &self,
        connection_id: i64,
        object_type: &str,
        object_name: &str,
        sha256: &str,
        now: i64,
    ) -> Result<Option<i64>> {
        let conn = self.open()?;
        // One statement, so two concurrent retries cannot both consume the approval.
        conn.query_row(
            "UPDATE mcp_apply_approvals SET status = 'consumed', consumed_at = ?5, ddl = ''
             WHERE id = (
                SELECT id FROM mcp_apply_approvals
                WHERE connection_id = ?1 AND object_type = ?2 AND object_name = ?3
                  AND sha256 = ?4 AND status = 'approved' AND expires_at > ?5
                ORDER BY id DESC LIMIT 1
             ) AND status = 'approved'
             RETURNING id",
            params![connection_id, object_type, object_name, sha256, now],
            |row| row.get(0),
        )
        .optional()
        .context("Failed to consume the deploy approval")
    }

    fn decide(&self, id: i64, approve: bool, now: i64, approved_until: i64) -> Result<bool> {
        let conn = self.open()?;
        let changed = if approve {
            conn.execute(
                "UPDATE mcp_apply_approvals
                 SET status = 'approved', decided_at = ?2, expires_at = ?3
                 WHERE id = ?1 AND status = 'pending' AND expires_at > ?2",
                params![id, now, approved_until],
            )?
        } else {
            conn.execute(
                "UPDATE mcp_apply_approvals
                 SET status = 'rejected', decided_at = ?2, ddl = ''
                 WHERE id = ?1 AND status = 'pending' AND expires_at > ?2",
                params![id, now],
            )?
        };
        Ok(changed == 1)
    }

    fn list_open(&self, now: i64) -> Result<Vec<ApplyRequest>> {
        let conn = self.open()?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM mcp_apply_approvals
             WHERE status IN ('pending', 'approved') AND expires_at > ?1
             ORDER BY id DESC"
        ))?;
        let rows = stmt
            .query_map([now], row_to_request)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    fn prune(&self, before: i64) -> Result<()> {
        self.open()?.execute(
            "DELETE FROM mcp_apply_approvals WHERE expires_at < ?1",
            [before],
        )?;
        Ok(())
    }
}
