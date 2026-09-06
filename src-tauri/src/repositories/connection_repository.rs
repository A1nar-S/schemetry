use std::path::PathBuf;

use anyhow::{Context, Result};
use keyring::{Entry, Error as KeyringError};
use rusqlite::{params, Connection, OptionalExtension};

use crate::models::{ConnectionRecord, DbType};

const KEYRING_SERVICE: &str = "schemetry";
const KEYRING_REF_MARKER: &str = "__KEYRING__";

fn db_type_str(db_type: DbType) -> &'static str {
    match db_type {
        DbType::Oracle => "oracle",
        DbType::Postgres => "postgres",
    }
}

fn parse_db_type(s: &str) -> DbType {
    match s {
        "postgres" => DbType::Postgres,
        _ => DbType::Oracle,
    }
}

/// SQLite has no `ADD COLUMN IF NOT EXISTS`; ignore the "duplicate column name" error
/// from a migration that's already been applied (e.g. on every subsequent app start).
fn add_column_if_missing(conn: &Connection, ddl: &str) {
    if let Err(e) = conn.execute(ddl, []) {
        if !e.to_string().contains("duplicate column name") {
            eprintln!("Failed to migrate connections table ({ddl}): {e}");
        }
    }
}

pub trait ConnectionRepository: Send + Sync {
    fn init_db(&self) -> Result<()>;
    fn get_all_connections(&self) -> Result<Vec<ConnectionRecord>>;
    fn insert_connection(&self, data: &ConnectionRecord) -> Result<()>;
    fn update_connection(&self, id: i64, data: &ConnectionRecord) -> Result<()>;
    fn delete_connection(&self, id: i64) -> Result<()>;
    /// Delete every connection and its keyring-stored password.
    fn delete_all_connections(&self) -> Result<()>;
    /// Insert if (group_name, name) is new; overwrite fields if it already exists.
    fn upsert_connection(&self, data: &ConnectionRecord) -> Result<()>;
}

pub struct SqliteConnectionRepository;

impl SqliteConnectionRepository {
    pub fn new() -> Self {
        Self
    }

    fn db_path() -> PathBuf {
        PathBuf::from("schemetry.db")
    }

    /// Keyring username: "group:name" — unique within group, disambiguates from other groups.
    fn keyring_key(group: &str, name: &str) -> String {
        format!("{}:{}", group, name)
    }

    fn save_password(key: &str, password: &str) -> Result<()> {
        Entry::new(KEYRING_SERVICE, key)
            .context("Failed to access OS credential manager")?
            .set_password(password)
            .with_context(|| format!("Failed to save password for key '{key}'"))
    }

    fn load_password(key: &str) -> Result<String> {
        Entry::new(KEYRING_SERVICE, key)
            .context("Failed to access OS credential manager")?
            .get_password()
            .with_context(|| format!("Failed to load password for key '{key}'"))
    }

    fn delete_password(key: &str) -> Result<()> {
        let entry = Entry::new(KEYRING_SERVICE, key).context("Failed to access keyring entry")?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("Failed to delete keyring entry '{key}'")),
        }
    }
}

impl ConnectionRepository for SqliteConnectionRepository {
    fn init_db(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS connections (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                name         TEXT    NOT NULL,
                host         TEXT    NOT NULL,
                port         INTEGER NOT NULL DEFAULT 1521,
                service_name TEXT    NOT NULL,
                username     TEXT    NOT NULL,
                keyring_ref  TEXT    NOT NULL,
                group_name   TEXT    NOT NULL DEFAULT '',
                UNIQUE(group_name, name)
            )",
            [],
        )?;
        add_column_if_missing(&conn, "ALTER TABLE connections ADD COLUMN db_type TEXT NOT NULL DEFAULT 'oracle'");
        add_column_if_missing(&conn, "ALTER TABLE connections ADD COLUMN pg_schema TEXT NOT NULL DEFAULT ''");
        Ok(())
    }

    fn get_all_connections(&self) -> Result<Vec<ConnectionRecord>> {
        let conn = Connection::open(Self::db_path())?;
        let mut stmt = conn.prepare(
            "SELECT id, name, host, port, service_name, username, keyring_ref, group_name, db_type, pg_schema
             FROM connections ORDER BY group_name, name",
        )?;

        let rows = stmt
            .query_map([], |row| {
                let port_i64: i64 = row.get(3)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    u16::try_from(port_i64).unwrap_or(1521),
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        let mut out = Vec::with_capacity(rows.len());
        for (id, name, host, port, service_name, username, _, group_name, db_type, pg_schema) in rows {
            let key = Self::keyring_key(&group_name, &name);
            let (password, password_broken) = match Self::load_password(&key) {
                Ok(p) => (p, false),
                Err(_) => (String::new(), true),
            };
            out.push(ConnectionRecord {
                id, name, host, port, service_name, username, password, group_name, password_broken,
                db_type: parse_db_type(&db_type), pg_schema,
            });
        }
        Ok(out)
    }

    fn insert_connection(&self, data: &ConnectionRecord) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;

        let exists: i64 = conn.query_row(
            "SELECT COUNT(1) FROM connections WHERE group_name = ?1 AND name = ?2",
            params![data.group_name, data.name],
            |row| row.get(0),
        )?;
        if exists > 0 {
            anyhow::bail!(
                "Connection '{}' already exists in group '{}'.",
                data.name, data.group_name
            );
        }

        let key = Self::keyring_key(&data.group_name, &data.name);
        Self::save_password(&key, &data.password)?;

        let result = conn.execute(
            "INSERT INTO connections (name, host, port, service_name, username, keyring_ref, group_name, db_type, pg_schema)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                data.name, data.host, data.port, data.service_name,
                data.username, KEYRING_REF_MARKER, data.group_name,
                db_type_str(data.db_type), data.pg_schema
            ],
        );

        if let Err(e) = result {
            let _ = Self::delete_password(&key);
            return Err(e).with_context(|| format!("Failed to insert connection '{}'", data.name));
        }

        Ok(())
    }

    fn update_connection(&self, id: i64, data: &ConnectionRecord) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;

        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT name, group_name FROM connections WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        let (old_name, old_group) =
            existing.ok_or_else(|| anyhow::anyhow!("Connection id={id} not found."))?;

        // Conflict check if name/group changed
        if data.name != old_name || data.group_name != old_group {
            let conflict: i64 = conn.query_row(
                "SELECT COUNT(1) FROM connections WHERE group_name = ?1 AND name = ?2 AND id != ?3",
                params![data.group_name, data.name, id],
                |row| row.get(0),
            )?;
            if conflict > 0 {
                anyhow::bail!(
                    "Connection '{}' already exists in group '{}'.",
                    data.name, data.group_name
                );
            }
        }

        let new_key = Self::keyring_key(&data.group_name, &data.name);
        Self::save_password(&new_key, &data.password)?;

        conn.execute(
            "UPDATE connections
             SET name=?1, host=?2, port=?3, service_name=?4, username=?5, keyring_ref=?6, group_name=?7, db_type=?8, pg_schema=?9
             WHERE id=?10",
            params![
                data.name, data.host, data.port, data.service_name,
                data.username, KEYRING_REF_MARKER, data.group_name,
                db_type_str(data.db_type), data.pg_schema, id
            ],
        )
        .with_context(|| format!("Failed to update connection id={id}"))?;

        let old_key = Self::keyring_key(&old_group, &old_name);
        if old_key != new_key {
            let _ = Self::delete_password(&old_key);
        }

        Ok(())
    }

    fn delete_connection(&self, id: i64) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;

        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT name, group_name FROM connections WHERE id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;

        if let Some((name, group)) = existing {
            conn.execute("DELETE FROM connections WHERE id = ?1", [id])?;
            let _ = Self::delete_password(&Self::keyring_key(&group, &name));
        }

        Ok(())
    }

    fn delete_all_connections(&self) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;

        let mut stmt = conn.prepare("SELECT name, group_name FROM connections")?;
        let keys = stmt
            .query_map([], |row| {
                Ok(Self::keyring_key(&row.get::<_, String>(1)?, &row.get::<_, String>(0)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);

        conn.execute("DELETE FROM connections", [])?;

        for key in keys {
            let _ = Self::delete_password(&key);
        }

        Ok(())
    }

    fn upsert_connection(&self, data: &ConnectionRecord) -> Result<()> {
        let conn = Connection::open(Self::db_path())?;

        let existing_id: Option<i64> = conn
            .query_row(
                "SELECT id FROM connections WHERE group_name = ?1 AND name = ?2",
                params![data.group_name, data.name],
                |row| row.get(0),
            )
            .optional()?;

        let key = Self::keyring_key(&data.group_name, &data.name);
        Self::save_password(&key, &data.password)?;

        if let Some(id) = existing_id {
            conn.execute(
                "UPDATE connections SET host=?1, port=?2, service_name=?3, username=?4, keyring_ref=?5, db_type=?6, pg_schema=?7
                 WHERE id=?8",
                params![
                    data.host, data.port, data.service_name, data.username, KEYRING_REF_MARKER,
                    db_type_str(data.db_type), data.pg_schema, id
                ],
            )
            .with_context(|| format!("Failed to overwrite connection '{}'", data.name))?;
        } else {
            conn.execute(
                "INSERT INTO connections (name, host, port, service_name, username, keyring_ref, group_name, db_type, pg_schema)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    data.name, data.host, data.port, data.service_name,
                    data.username, KEYRING_REF_MARKER, data.group_name,
                    db_type_str(data.db_type), data.pg_schema
                ],
            )
            .with_context(|| format!("Failed to insert connection '{}'", data.name))?;
        }

        Ok(())
    }
}

