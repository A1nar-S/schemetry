use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::models::QueryHistoryEntry;

pub trait QueryHistoryRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    fn load_history(&self) -> Result<Vec<QueryHistoryEntry>>;
    fn append_query(&self, sql: &str) -> Result<()>;
    fn delete_query(&self, id: i64) -> Result<()>;
    fn set_favorite(&self, id: i64, favorite: bool, description: &str) -> Result<()>;
    fn reorder_favorites(&self, ordered_ids: &[i64]) -> Result<()>;
    fn clear_history(&self) -> Result<()>;
}

pub struct SqliteQueryHistoryRepository;

/// Pinning was removed in favor of favorites. Queries pinned in older versions
/// become favorites (after existing ones, newest first) so "Clear" still keeps them.
fn migrate_pinned_to_favorites(conn: &Connection) -> Result<()> {
    let first_order: i64 = conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1) + 1 FROM query_history WHERE favorite = 1",
        [],
        |row| row.get(0),
    )?;
    let pinned_ids = conn
        .prepare("SELECT id FROM query_history WHERE pinned = 1 AND favorite = 0 ORDER BY id DESC")?
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (i, id) in pinned_ids.into_iter().enumerate() {
        conn.execute(
            "UPDATE query_history SET favorite = 1, sort_order = ?1 WHERE id = ?2",
            params![first_order + i as i64, id],
        )?;
    }
    conn.execute("UPDATE query_history SET pinned = 0 WHERE pinned = 1", [])?;
    Ok(())
}

impl SqliteQueryHistoryRepository {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> PathBuf {
        crate::utils::paths::db_path()
    }
}

impl QueryHistoryRepository for SqliteQueryHistoryRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "
            CREATE TABLE IF NOT EXISTS query_history (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                sql_text   TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            )
            ",
            [],
        )?;
        let _ = conn.execute(
            "ALTER TABLE query_history ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE query_history ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE query_history ADD COLUMN description TEXT NOT NULL DEFAULT ''",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE query_history ADD COLUMN sort_order INTEGER NOT NULL DEFAULT 0",
            [],
        );
        migrate_pinned_to_favorites(&conn)?;
        Ok(())
    }

    fn load_history(&self) -> Result<Vec<QueryHistoryEntry>> {
        let conn = Connection::open(Self::db_path())?;
        let mut stmt = conn.prepare(
            "SELECT id, sql_text, favorite, description FROM query_history ORDER BY favorite DESC, sort_order ASC, id DESC",
        )?;

        let history = stmt
            .query_map([], |row| {
                Ok(QueryHistoryEntry {
                    id: row.get::<_, i64>(0)?,
                    sql_text: row.get::<_, String>(1)?,
                    favorite: row.get::<_, i64>(2)? != 0,
                    description: row.get::<_, String>(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(history)
    }

    fn append_query(&self, sql: &str) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        let last: Option<String> = conn
            .query_row(
                "SELECT sql_text FROM query_history ORDER BY id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if last.as_deref() == Some(sql) {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO query_history (sql_text) VALUES (?1)",
            params![sql],
        )?;
        Ok(())
    }

    fn delete_query(&self, id: i64) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute("DELETE FROM query_history WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn set_favorite(&self, id: i64, favorite: bool, description: &str) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        if favorite {
            let max_order: i64 = conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM query_history WHERE favorite = 1",
                [],
                |row| row.get(0),
            )?;
            conn.execute(
                "UPDATE query_history SET favorite = 1, description = ?1, sort_order = ?2 WHERE id = ?3",
                params![description, max_order + 1, id],
            )?;
        } else {
            conn.execute(
                "UPDATE query_history SET favorite = 0, description = '', sort_order = 0 WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    fn reorder_favorites(&self, ordered_ids: &[i64]) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        for (i, &id) in ordered_ids.iter().enumerate() {
            conn.execute(
                "UPDATE query_history SET sort_order = ?1 WHERE id = ?2",
                params![i as i64, id],
            )?;
        }
        Ok(())
    }

    fn clear_history(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute("DELETE FROM query_history WHERE favorite = 0", [])?;
        Ok(())
    }
}
