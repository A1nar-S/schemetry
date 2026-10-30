use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::{DbPathResolution, move_legacy_database, resolve_db_path};

#[test]
fn cli_override_takes_precedence_over_environment_and_app_data() {
    let resolution = resolve_db_path(
        Some(OsStr::new("cli.db")),
        Some(OsStr::new("env.db")),
        Path::new("app-data"),
        Path::new("workspace"),
    );

    assert_eq!(
        resolution,
        DbPathResolution {
            path: PathBuf::from("workspace/cli.db"),
            migrate_legacy: false,
        }
    );
}

#[test]
fn environment_override_takes_precedence_over_app_data() {
    let resolution = resolve_db_path(
        None,
        Some(OsStr::new("env.db")),
        Path::new("app-data"),
        Path::new("workspace"),
    );

    assert_eq!(resolution.path, PathBuf::from("workspace/env.db"));
    assert!(!resolution.migrate_legacy);
}

#[test]
fn default_path_uses_app_data_and_enables_legacy_migration() {
    let resolution = resolve_db_path(
        None,
        None,
        Path::new("app-data/lv.schemetry.app"),
        Path::new("workspace"),
    );

    assert_eq!(
        resolution.path,
        PathBuf::from("app-data/lv.schemetry.app/schemetry.db")
    );
    assert!(resolution.migrate_legacy);
}

#[test]
fn relative_override_is_resolved_from_working_directory() {
    let resolution = resolve_db_path(
        Some(OsStr::new("data/db.sqlite")),
        None,
        Path::new("app-data"),
        Path::new("workspace"),
    );

    assert_eq!(resolution.path, PathBuf::from("workspace/data/db.sqlite"));
}

#[test]
fn legacy_database_is_moved_when_destination_is_missing() {
    let temp_dir = tempfile::tempdir().unwrap();
    let source = temp_dir.path().join("legacy.db");
    let destination = temp_dir.path().join("data").join("schemetry.db");
    fs::write(&source, b"sqlite contents").unwrap();

    move_legacy_database(&source, &destination).unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read(destination).unwrap(), b"sqlite contents");
}

#[test]
fn legacy_database_is_not_overwritten() {
    let temp_dir = tempfile::tempdir().unwrap();
    let source = temp_dir.path().join("legacy.db");
    let destination = temp_dir.path().join("schemetry.db");
    fs::write(&source, b"legacy").unwrap();
    fs::write(&destination, b"existing").unwrap();

    move_legacy_database(&source, &destination).unwrap();

    assert_eq!(fs::read(&source).unwrap(), b"legacy");
    assert_eq!(fs::read(&destination).unwrap(), b"existing");
}
