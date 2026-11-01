use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use keyring::{Entry, Error as KeyringError};
use rusqlite::{Connection, OptionalExtension, params};

use crate::models::{McpDataAccess, McpPolicy};

const KEYRING_SERVICE: &str = "schemetry";

pub trait McpPolicyRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    /// The stored policy, or `None` when the connection never had one saved.
    fn get(&self, connection_id: i64) -> Result<Option<McpPolicy>>;
    fn list_all(&self) -> Result<HashMap<i64, McpPolicy>>;
    /// Saves everything except the dedicated-account password.
    fn upsert(&self, policy: &McpPolicy) -> Result<()>;
    fn delete(&self, connection_id: i64) -> Result<()>;
    fn delete_all(&self) -> Result<()>;
    fn save_account_password(&self, connection_id: i64, password: &str) -> Result<()>;
    fn load_account_password(&self, connection_id: i64) -> Result<Option<String>>;
    fn delete_account_password(&self, connection_id: i64) -> Result<()>;
}

pub struct SqliteMcpPolicyRepository {
    /// Overrides the app-wide database path (used by tests for isolated files).
    path: Option<PathBuf>,
}

impl SqliteMcpPolicyRepository {
    pub fn new() -> Self {
        Self { path: None }
    }

    pub fn with_path(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn db_path(&self) -> PathBuf {
        self.path
            .clone()
            .unwrap_or_else(crate::utils::paths::db_path)
    }

    fn keyring_key(connection_id: i64) -> String {
        format!("mcp-account:{connection_id}")
    }

    fn account_password_exists(connection_id: i64) -> bool {
        Entry::new(KEYRING_SERVICE, &Self::keyring_key(connection_id))
            .and_then(|entry| entry.get_password())
            .is_ok()
    }
}

impl Default for SqliteMcpPolicyRepository {
    fn default() -> Self {
        Self::new()
    }
}

const COLUMNS: &str = "connection_id, mcp_enabled, allow_schema_list, allow_ddl_read, \
    allow_save_scripts, allow_compare_fix, data_access, allow_raw_sql, table_rules, \
    object_deny, max_rows, query_timeout_secs, mcp_username, allow_invalid_objects, \
    allow_compare_object, allow_apply_ddl, apply_ddl_requires_approval";

fn row_to_policy(row: &rusqlite::Row) -> rusqlite::Result<McpPolicy> {
    let data_access: String = row.get(6)?;
    let table_rules: String = row.get(8)?;
    let object_deny: String = row.get(9)?;
    let max_rows: i64 = row.get(10)?;
    let timeout: i64 = row.get(11)?;
    Ok(McpPolicy {
        connection_id: row.get(0)?,
        mcp_enabled: row.get(1)?,
        allow_schema_list: row.get(2)?,
        allow_ddl_read: row.get(3)?,
        allow_save_scripts: row.get(4)?,
        allow_compare_fix: row.get(5)?,
        allow_compare_object: row.get(14)?,
        // Anything unrecognised falls back to the most restrictive value.
        data_access: if data_access == "allowlist" {
            McpDataAccess::Allowlist
        } else {
            McpDataAccess::None
        },
        allow_raw_sql: row.get(7)?,
        table_rules: serde_json::from_str(&table_rules).unwrap_or_default(),
        object_deny: serde_json::from_str(&object_deny).unwrap_or_default(),
        max_rows: u32::try_from(max_rows).unwrap_or(McpPolicy::DEFAULT_MAX_ROWS),
        query_timeout_secs: u32::try_from(timeout).unwrap_or(McpPolicy::DEFAULT_TIMEOUT_SECS),
        mcp_username: row.get(12)?,
        allow_invalid_objects: row.get(13)?,
        mcp_password_set: false,
        allow_apply_ddl: row.get(15)?,
        apply_ddl_requires_approval: row.get(16)?,
    })
}

impl McpPolicyRepository for SqliteMcpPolicyRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(self.db_path())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS connection_mcp_policy (
                connection_id       INTEGER PRIMARY KEY,
                mcp_enabled         INTEGER NOT NULL DEFAULT 0,
                allow_schema_list   INTEGER NOT NULL DEFAULT 1,
                allow_ddl_read      INTEGER NOT NULL DEFAULT 1,
                allow_save_scripts  INTEGER NOT NULL DEFAULT 0,
                allow_compare_fix   INTEGER NOT NULL DEFAULT 0,
                data_access         TEXT    NOT NULL DEFAULT 'none',
                allow_raw_sql       INTEGER NOT NULL DEFAULT 0,
                table_rules         TEXT    NOT NULL DEFAULT '[]',
                object_deny         TEXT    NOT NULL DEFAULT '[]',
                max_rows            INTEGER NOT NULL DEFAULT 200,
                query_timeout_secs  INTEGER NOT NULL DEFAULT 30,
                mcp_username        TEXT    NOT NULL DEFAULT ''
            )",
            [],
        )?;
        // Added after the first release: existing policies stay denied (default-deny).
        let has_invalid_objects_column = conn
            .prepare("SELECT 1 FROM pragma_table_info('connection_mcp_policy') WHERE name = 'allow_invalid_objects'")?
            .exists([])?;
        if !has_invalid_objects_column {
            conn.execute(
                "ALTER TABLE connection_mcp_policy \
                 ADD COLUMN allow_invalid_objects INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        let has_compare_object_column = conn
            .prepare("SELECT 1 FROM pragma_table_info('connection_mcp_policy') WHERE name = 'allow_compare_object'")?
            .exists([])?;
        if !has_compare_object_column {
            conn.execute(
                "ALTER TABLE connection_mcp_policy ADD COLUMN allow_compare_object INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }

        // Deploys stay off for connections that predate the capability.
        for ddl in [
            "ALTER TABLE connection_mcp_policy ADD COLUMN allow_apply_ddl INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE connection_mcp_policy ADD COLUMN apply_ddl_requires_approval INTEGER NOT NULL DEFAULT 1",
        ] {
            if let Err(e) = conn.execute(ddl, [])
                && !e.to_string().contains("duplicate column name")
            {
                return Err(e).context("Failed to migrate the MCP policy table");
            }
        }
        Ok(())
    }

    fn get(&self, connection_id: i64) -> Result<Option<McpPolicy>> {
        let conn = Connection::open(self.db_path())?;
        let policy = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM connection_mcp_policy WHERE connection_id = ?1"),
                [connection_id],
                row_to_policy,
            )
            .optional()?;
        Ok(policy.map(|mut p| {
            p.mcp_password_set =
                !p.mcp_username.is_empty() && Self::account_password_exists(connection_id);
            p
        }))
    }

    fn list_all(&self) -> Result<HashMap<i64, McpPolicy>> {
        let conn = Connection::open(self.db_path())?;
        let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM connection_mcp_policy"))?;
        let policies = stmt
            .query_map([], row_to_policy)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(policies
            .into_iter()
            .map(|mut p| {
                p.mcp_password_set =
                    !p.mcp_username.is_empty() && Self::account_password_exists(p.connection_id);
                (p.connection_id, p)
            })
            .collect())
    }

    fn upsert(&self, policy: &McpPolicy) -> Result<()> {
        let conn = Connection::open(self.db_path())?;
        let data_access = match policy.data_access {
            McpDataAccess::None => "none",
            McpDataAccess::Allowlist => "allowlist",
        };
        conn.execute(
            "INSERT INTO connection_mcp_policy (
                connection_id, mcp_enabled, allow_schema_list, allow_ddl_read, allow_save_scripts,
                allow_compare_fix, data_access, allow_raw_sql, table_rules, object_deny,
                max_rows, query_timeout_secs, mcp_username, allow_invalid_objects, allow_compare_object, allow_apply_ddl,
                apply_ddl_requires_approval
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(connection_id) DO UPDATE SET
                mcp_enabled = excluded.mcp_enabled,
                allow_schema_list = excluded.allow_schema_list,
                allow_ddl_read = excluded.allow_ddl_read,
                allow_save_scripts = excluded.allow_save_scripts,
                allow_compare_fix = excluded.allow_compare_fix,
                data_access = excluded.data_access,
                allow_raw_sql = excluded.allow_raw_sql,
                table_rules = excluded.table_rules,
                object_deny = excluded.object_deny,
                max_rows = excluded.max_rows,
                query_timeout_secs = excluded.query_timeout_secs,
                mcp_username = excluded.mcp_username,
                allow_invalid_objects = excluded.allow_invalid_objects,
                allow_compare_object = excluded.allow_compare_object,
                allow_apply_ddl = excluded.allow_apply_ddl,
                apply_ddl_requires_approval = excluded.apply_ddl_requires_approval",
            params![
                policy.connection_id,
                policy.mcp_enabled,
                policy.allow_schema_list,
                policy.allow_ddl_read,
                policy.allow_save_scripts,
                policy.allow_compare_fix,
                data_access,
                policy.allow_raw_sql,
                serde_json::to_string(&policy.table_rules)?,
                serde_json::to_string(&policy.object_deny)?,
                policy.max_rows,
                policy.query_timeout_secs,
                policy.mcp_username,
                policy.allow_invalid_objects,
                policy.allow_compare_object,
                policy.allow_apply_ddl,
                policy.apply_ddl_requires_approval,
            ],
        )
        .with_context(|| {
            format!(
                "Failed to save MCP policy for connection {}",
                policy.connection_id
            )
        })?;
        Ok(())
    }

    fn delete(&self, connection_id: i64) -> Result<()> {
        let conn = Connection::open(self.db_path())?;
        conn.execute(
            "DELETE FROM connection_mcp_policy WHERE connection_id = ?1",
            [connection_id],
        )?;
        Ok(())
    }

    fn delete_all(&self) -> Result<()> {
        let conn = Connection::open(self.db_path())?;
        conn.execute("DELETE FROM connection_mcp_policy", [])?;
        Ok(())
    }

    fn save_account_password(&self, connection_id: i64, password: &str) -> Result<()> {
        Entry::new(KEYRING_SERVICE, &Self::keyring_key(connection_id))
            .context("Failed to access OS credential manager")?
            .set_password(password)
            .context("Failed to save the MCP account password")
    }

    fn load_account_password(&self, connection_id: i64) -> Result<Option<String>> {
        let entry = Entry::new(KEYRING_SERVICE, &Self::keyring_key(connection_id))
            .context("Failed to access OS credential manager")?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(e).context("Failed to load the MCP account password"),
        }
    }

    fn delete_account_password(&self, connection_id: i64) -> Result<()> {
        let entry = Entry::new(KEYRING_SERVICE, &Self::keyring_key(connection_id))
            .context("Failed to access OS credential manager")?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e).context("Failed to delete the MCP account password"),
        }
    }
}

#[cfg(test)]
#[path = "tests/mcp_policy_repository.rs"]
mod tests;
