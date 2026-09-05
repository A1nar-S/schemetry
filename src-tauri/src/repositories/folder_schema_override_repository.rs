use std::path::PathBuf;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::SchemaFolderOverride;

pub trait SchemaFolderOverrideRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>>;
    fn insert_override(&self, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride>;
    fn update_override(&self, id: i64, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride>;
    fn delete_override(&self, id: i64) -> Result<()>;
    /// The override row for a schema, matched case-insensitively, if one is set.
    fn find_for_schema(&self, schema_name: &str) -> Result<Option<SchemaFolderOverride>>;
}

pub struct SqliteSchemaFolderOverrideRepository;

impl SqliteSchemaFolderOverrideRepository {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> PathBuf {
        PathBuf::from("schemetry.db")
    }
}

const COLUMNS: &str = "id, schema_name, folder_path, encoding, extensions, storage_modes, naming_convention, \
    code_folder_name, migration_folder_name, migration_folder_mode, migration_version_label";

fn row_to_override(row: &rusqlite::Row) -> rusqlite::Result<SchemaFolderOverride> {
    Ok(SchemaFolderOverride {
        id: row.get(0)?,
        schema_name: row.get(1)?,
        folder_path: row.get(2)?,
        encoding: row.get(3)?,
        extensions: row.get(4)?,
        storage_modes: row.get(5)?,
        naming_convention: row.get(6)?,
        code_folder_name: row.get(7)?,
        migration_folder_name: row.get(8)?,
        migration_folder_mode: row.get(9)?,
        migration_version_label: row.get(10)?,
    })
}

impl SchemaFolderOverrideRepository for SqliteSchemaFolderOverrideRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS folder_schema_overrides (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                schema_name TEXT    NOT NULL UNIQUE,
                folder_path TEXT    NOT NULL DEFAULT ''
            )",
            [],
        )?;
        for col in [
            "encoding",
            "extensions",
            "storage_modes",
            "naming_convention",
            "code_folder_name",
            "migration_folder_name",
            "migration_folder_mode",
            "migration_version_label",
        ] {
            let _ = conn.execute(
                &format!("ALTER TABLE folder_schema_overrides ADD COLUMN {col} TEXT NOT NULL DEFAULT ''"),
                [],
            );
        }
        Ok(())
    }

    fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>> {
        let conn = Connection::open(Self::db_path())?;
        let mut stmt =
            conn.prepare(&format!("SELECT {COLUMNS} FROM folder_schema_overrides ORDER BY schema_name"))?;

        let rows = stmt
            .query_map([], row_to_override)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(rows)
    }

    fn insert_override(&self, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "INSERT INTO folder_schema_overrides
                (schema_name, folder_path, encoding, extensions, storage_modes, naming_convention,
                 code_folder_name, migration_folder_name, migration_folder_mode, migration_version_label)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                data.schema_name,
                data.folder_path,
                data.encoding,
                data.extensions,
                data.storage_modes,
                data.naming_convention,
                data.code_folder_name,
                data.migration_folder_name,
                data.migration_folder_mode,
                data.migration_version_label
            ],
        )
        .context("Failed to insert schema folder override")?;

        let id = conn.last_insert_rowid();
        Ok(SchemaFolderOverride { id, ..data.clone() })
    }

    fn update_override(&self, id: i64, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        let conn = Connection::open(Self::db_path())?;

        let exists: Option<i64> = conn
            .query_row("SELECT id FROM folder_schema_overrides WHERE id = ?1", [id], |row| row.get(0))
            .optional()?;
        exists.ok_or_else(|| anyhow::anyhow!("Schema folder override id={id} not found."))?;

        conn.execute(
            "UPDATE folder_schema_overrides SET
                schema_name=?1, folder_path=?2, encoding=?3, extensions=?4, storage_modes=?5,
                naming_convention=?6, code_folder_name=?7, migration_folder_name=?8,
                migration_folder_mode=?9, migration_version_label=?10
             WHERE id=?11",
            params![
                data.schema_name,
                data.folder_path,
                data.encoding,
                data.extensions,
                data.storage_modes,
                data.naming_convention,
                data.code_folder_name,
                data.migration_folder_name,
                data.migration_folder_mode,
                data.migration_version_label,
                id
            ],
        )
        .with_context(|| format!("Failed to update schema folder override id={id}"))?;

        Ok(SchemaFolderOverride { id, ..data.clone() })
    }

    fn delete_override(&self, id: i64) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute("DELETE FROM folder_schema_overrides WHERE id = ?1", [id])?;
        Ok(())
    }

    fn find_for_schema(&self, schema_name: &str) -> Result<Option<SchemaFolderOverride>> {
        let conn = Connection::open(Self::db_path())?;
        conn.query_row(
            &format!("SELECT {COLUMNS} FROM folder_schema_overrides WHERE lower(schema_name) = lower(?1)"),
            params![schema_name],
            row_to_override,
        )
        .optional()
        .context("Failed to look up schema folder override")
    }
}
