use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};

const DATABASE_FILE_NAME: &str = "schemetry.db";
const APP_IDENTIFIER: &str = "lv.schemetry.app";
const DATABASE_ENV_VAR: &str = "SCHEMETRY_DB";

static DATABASE_PATH: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
struct DbPathResolution {
    path: PathBuf,
    migrate_legacy: bool,
}

fn absolute_path(path: &Path, working_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    }
}

fn resolve_db_path(
    cli_override: Option<&OsStr>,
    env_override: Option<&OsStr>,
    app_data_dir: &Path,
    working_dir: &Path,
) -> DbPathResolution {
    let override_path = cli_override
        .filter(|value| !value.is_empty())
        .or_else(|| env_override.filter(|value| !value.is_empty()));

    match override_path {
        Some(path) => DbPathResolution {
            path: absolute_path(Path::new(path), working_dir),
            migrate_legacy: false,
        },
        None => DbPathResolution {
            path: app_data_dir.join(DATABASE_FILE_NAME),
            migrate_legacy: true,
        },
    }
}

fn app_data_dir() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|path| path.join(APP_IDENTIFIER))
        .ok_or_else(|| anyhow!("Could not determine the operating system app-data directory"))
}

fn move_legacy_database(source: &Path, destination: &Path) -> Result<()> {
    if source == destination || !source.is_file() || destination.exists() {
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Could not create database directory {}", parent.display()))?;
    }

    match fs::hard_link(source, destination) {
        Ok(()) => {
            fs::remove_file(source).with_context(|| {
                format!(
                    "Moved legacy database but could not remove {}",
                    source.display()
                )
            })?;
            Ok(())
        }
        Err(_) => {
            let mut input = File::open(source)
                .with_context(|| format!("Could not open legacy database {}", source.display()))?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination)
                .with_context(|| {
                    format!("Could not create database at {}", destination.display())
                })?;

            if let Err(error) = io::copy(&mut input, &mut output).and_then(|_| output.sync_all()) {
                drop(output);
                let _ = fs::remove_file(destination);
                return Err(error).with_context(|| {
                    format!(
                        "Could not copy legacy database to {}",
                        destination.display()
                    )
                });
            }

            drop(output);
            fs::remove_file(source).with_context(|| {
                format!(
                    "Copied legacy database but could not remove {}",
                    source.display()
                )
            })?;
            Ok(())
        }
    }
}

fn initialize_path(path: PathBuf, migrate_legacy: bool, working_dir: &Path) -> Result<PathBuf> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Could not create database directory {}", parent.display()))?;
    }

    if migrate_legacy {
        move_legacy_database(&working_dir.join(DATABASE_FILE_NAME), &path)?;
    }

    if let Some(configured) = DATABASE_PATH.get() {
        if configured != &path {
            bail!(
                "Database path is already initialized as {}; cannot change it to {}",
                configured.display(),
                path.display()
            );
        }
        return Ok(configured.clone());
    }

    match DATABASE_PATH.set(path.clone()) {
        Ok(()) => Ok(path),
        Err(_) => {
            let configured = DATABASE_PATH.get().expect("database path was just set");
            if configured == &path {
                Ok(configured.clone())
            } else {
                bail!(
                    "Database path is already initialized as {}; cannot change it to {}",
                    configured.display(),
                    path.display()
                )
            }
        }
    }
}

pub fn configure_db_path(cli_override: Option<OsString>) -> Result<PathBuf> {
    let working_dir = std::env::current_dir().context("Could not determine current directory")?;
    let env_override = std::env::var_os(DATABASE_ENV_VAR);
    let resolution = resolve_db_path(
        cli_override.as_deref(),
        env_override.as_deref(),
        &app_data_dir()?,
        &working_dir,
    );

    initialize_path(resolution.path, resolution.migrate_legacy, &working_dir)
}

pub fn db_path() -> PathBuf {
    DATABASE_PATH.get().cloned().unwrap_or_else(|| {
        app_data_dir()
            .expect("could not determine the operating system app-data directory")
            .join(DATABASE_FILE_NAME)
    })
}

#[cfg(test)]
#[path = "tests/paths.rs"]
mod tests;
