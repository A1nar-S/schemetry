use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{params, Connection};

fn db_path() -> PathBuf {
    PathBuf::from("schemetry.db")
}

pub fn init_db() -> Result<()> {
    let conn = Connection::open(db_path())?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL DEFAULT ''
        );",
    )?;
    Ok(())
}

fn load_setting(key: &str) -> Option<String> {
    let conn = Connection::open(db_path()).ok()?;
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|v| !v.trim().is_empty())
}

fn save_setting(key: &str, value: &str) -> Result<()> {
    let conn = Connection::open(db_path())?;
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn load_output_folder() -> Option<String> {
    load_setting("output_folder")
}

pub fn save_output_folder(folder: &str) -> Result<()> {
    save_setting("output_folder", folder)
}

pub fn load_client_lib_dir() -> Option<String> {
    load_setting("client_lib_dir").or_else(|| {
        std::env::var("ORACLE_CLIENT_LIB_DIR")
            .ok()
            .filter(|v| !v.trim().is_empty())
    })
}

pub fn save_client_lib_dir(dir: &str) -> Result<()> {
    save_setting("client_lib_dir", dir)
}

pub fn load_last_query_export_dir() -> Option<String> {
    load_setting("last_query_export_dir")
}

pub fn save_last_query_export_dir(dir: &str) -> Result<()> {
    save_setting("last_query_export_dir", dir)
}

pub fn load_schema_root_folder() -> Option<String> {
    load_setting("schema_root_folder")
}

pub fn save_schema_root_folder(folder: &str) -> Result<()> {
    save_setting("schema_root_folder", folder)
}

pub fn load_ddl_file_encoding() -> Option<String> {
    load_setting("ddl_file_encoding")
}

pub fn save_ddl_file_encoding(encoding: &str) -> Result<()> {
    save_setting("ddl_file_encoding", encoding)
}

pub fn load_plsql_dev_path() -> Option<String> {
    load_setting("plsql_dev_path")
}

pub fn save_plsql_dev_path(path: &str) -> Result<()> {
    save_setting("plsql_dev_path", path)
}

/// Fallback DDL file extension per Oracle object type, used when neither the global
/// setting nor a schema override has a value for that type. Mirrors what Oracle's own
/// DDL export tools (SQL Developer, PL/SQL Developer) commonly use.
pub const DEFAULT_DDL_EXTENSIONS: &[(&str, &str)] = &[
    ("TABLE", "tab"),
    ("VIEW", "vw"),
    ("MATERIALIZED VIEW", "mv"),
    ("PROCEDURE", "prc"),
    ("FUNCTION", "fnc"),
    ("PACKAGE", "pck"),
    ("PACKAGE BODY", "pkb"),
    ("TRIGGER", "trg"),
    ("SEQUENCE", "seq"),
    ("SYNONYM", "syn"),
    ("TYPE", "typ"),
    ("JOB", "job"),
];

/// The extension to use for an object type with no configured override.
pub fn default_ddl_extension(object_type: &str) -> &'static str {
    DEFAULT_DDL_EXTENSIONS
        .iter()
        .find(|(t, _)| *t == object_type.trim().to_ascii_uppercase())
        .map(|(_, ext)| *ext)
        .unwrap_or("sql")
}

/// The global DDL extension overrides, keyed by upper-cased object type. Merged over
/// `DEFAULT_DDL_EXTENSIONS` so the map always has an entry for every known type.
pub fn load_ddl_extensions() -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = DEFAULT_DDL_EXTENSIONS
        .iter()
        .map(|(t, ext)| (t.to_string(), ext.to_string()))
        .collect();
    if let Some(saved) = load_setting("ddl_file_extensions") {
        if let Ok(overrides) = serde_json::from_str::<std::collections::HashMap<String, String>>(&saved) {
            for (k, v) in overrides {
                if !v.trim().is_empty() {
                    map.insert(k.to_ascii_uppercase(), v.trim().to_string());
                }
            }
        }
    }
    map
}

pub fn save_ddl_extensions(map: &std::collections::HashMap<String, String>) -> Result<()> {
    let json = serde_json::to_string(map)?;
    save_setting("ddl_file_extensions", &json)
}

/// The DDL file extension for a given object type, honoring the global setting and
/// falling back to the built-in default.
pub fn resolve_ddl_extension(object_type: &str) -> String {
    let key = object_type.trim().to_ascii_uppercase();
    load_ddl_extensions()
        .get(&key)
        .cloned()
        .unwrap_or_else(|| default_ddl_extension(&key).to_string())
}

/// The global storage-mode overrides, keyed by upper-cased object type. A type missing
/// from the map defaults to `"both"` (the historical behavior: write both a raw code
/// file and an idempotent migration file).
pub fn load_storage_modes() -> std::collections::HashMap<String, String> {
    load_setting("ddl_storage_modes")
        .and_then(|saved| serde_json::from_str::<std::collections::HashMap<String, String>>(&saved).ok())
        .map(|map| {
            map.into_iter()
                .map(|(k, v)| (k.to_ascii_uppercase(), v))
                .filter(|(_, v)| !v.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_storage_modes(map: &std::collections::HashMap<String, String>) -> Result<()> {
    let json = serde_json::to_string(map)?;
    save_setting("ddl_storage_modes", &json)
}

/// The storage mode ("code", "migration", or "both") for a given object type, honoring
/// the global setting and falling back to `"both"`.
pub fn resolve_storage_mode(object_type: &str) -> String {
    let key = object_type.trim().to_ascii_uppercase();
    load_storage_modes().get(&key).cloned().unwrap_or_else(|| "both".to_string())
}

pub fn load_naming_convention() -> Option<String> {
    load_setting("ddl_naming_convention")
}

pub fn save_naming_convention(convention: &str) -> Result<()> {
    save_setting("ddl_naming_convention", convention)
}

/// How the migration subfolder is chosen: `"year"` (the calendar year, e.g.
/// `migration/2026/`) or `"version"` (the manually-set `migration_version_label`, e.g.
/// `migration/1.4/`).
pub fn load_migration_folder_mode() -> Option<String> {
    load_setting("migration_folder_mode")
}

pub fn save_migration_folder_mode(mode: &str) -> Result<()> {
    save_setting("migration_folder_mode", mode)
}

/// The manually-set folder name used when `migration_folder_mode` is `"version"`.
pub fn load_migration_version_label() -> Option<String> {
    load_setting("migration_version_label")
}

pub fn save_migration_version_label(label: &str) -> Result<()> {
    save_setting("migration_version_label", label)
}

/// The subfolder name (under each schema's folder root) that raw code DDL files are
/// written to. Defaults to `"code"`.
pub fn load_code_folder_name() -> Option<String> {
    load_setting("code_folder_name")
}

pub fn save_code_folder_name(name: &str) -> Result<()> {
    save_setting("code_folder_name", name)
}

/// The subfolder name (under each schema's folder root) that migration scripts are
/// written to. Defaults to `"migration"`, matching Flyway's own default layout.
pub fn load_migration_folder_name() -> Option<String> {
    load_setting("migration_folder_name")
}

pub fn save_migration_folder_name(name: &str) -> Result<()> {
    save_setting("migration_folder_name", name)
}
