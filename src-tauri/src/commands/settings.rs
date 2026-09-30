use std::collections::HashMap;

use serde::Serialize;

use crate::repositories::oracle_repository::{configure_client_lib_dir, set_client_lib_dir_hint};
use crate::services;

#[derive(Serialize)]
pub struct AppSettings {
    pub output_folder: String,
    pub client_lib_dir: String,
    pub last_query_export_dir: String,
    pub schema_root_folder: String,
    pub ddl_file_encoding: String,
    pub ddl_file_extensions: HashMap<String, String>,
    pub storage_modes: HashMap<String, String>,
    pub naming_convention: String,
    pub code_folder_name: String,
    pub migration_folder_name: String,
    pub migration_folder_mode: String,
    pub migration_version_label: String,
    pub plsql_dev_path: String,
}

#[derive(Serialize)]
pub struct SaveSettingsResponse {
    pub oracle_client_initialized: bool,
}

#[tauri::command]
pub fn get_settings() -> AppSettings {
    AppSettings {
        output_folder: services::settings::load_output_folder().unwrap_or_default(),
        client_lib_dir: services::settings::load_client_lib_dir().unwrap_or_default(),
        last_query_export_dir: services::settings::load_last_query_export_dir().unwrap_or_default(),
        schema_root_folder: services::settings::load_schema_root_folder().unwrap_or_default(),
        ddl_file_encoding: services::settings::load_ddl_file_encoding().unwrap_or_else(|| "utf8".to_string()),
        ddl_file_extensions: services::settings::load_ddl_extensions(),
        storage_modes: services::settings::load_storage_modes(),
        naming_convention: services::settings::load_naming_convention().unwrap_or_else(|| "timestamp".to_string()),
        code_folder_name: services::settings::load_code_folder_name().unwrap_or_else(|| "code".to_string()),
        migration_folder_name: services::settings::load_migration_folder_name().unwrap_or_else(|| "migration".to_string()),
        migration_folder_mode: services::settings::load_migration_folder_mode().unwrap_or_else(|| "year".to_string()),
        migration_version_label: services::settings::load_migration_version_label().unwrap_or_default(),
        plsql_dev_path: services::settings::load_plsql_dev_path().unwrap_or_default(),
    }
}

#[tauri::command]
pub fn save_settings(
    output_folder: String,
    client_lib_dir: String,
    schema_root_folder: String,
    ddl_file_encoding: String,
    ddl_file_extensions: HashMap<String, String>,
    storage_modes: HashMap<String, String>,
    naming_convention: String,
    code_folder_name: String,
    migration_folder_name: String,
    migration_folder_mode: String,
    migration_version_label: String,
    plsql_dev_path: String,
) -> Result<SaveSettingsResponse, String> {
    services::settings::save_output_folder(output_folder.trim()).map_err(|e| e.to_string())?;
    services::settings::save_client_lib_dir(client_lib_dir.trim()).map_err(|e| e.to_string())?;
    services::settings::save_schema_root_folder(schema_root_folder.trim()).map_err(|e| e.to_string())?;
    services::settings::save_ddl_file_encoding(ddl_file_encoding.trim()).map_err(|e| e.to_string())?;
    services::settings::save_ddl_extensions(&ddl_file_extensions).map_err(|e| e.to_string())?;
    services::settings::save_storage_modes(&storage_modes).map_err(|e| e.to_string())?;
    services::settings::save_naming_convention(naming_convention.trim()).map_err(|e| e.to_string())?;
    services::settings::save_code_folder_name(code_folder_name.trim()).map_err(|e| e.to_string())?;
    services::settings::save_migration_folder_name(migration_folder_name.trim()).map_err(|e| e.to_string())?;
    services::settings::save_migration_folder_mode(migration_folder_mode.trim()).map_err(|e| e.to_string())?;
    services::settings::save_migration_version_label(migration_version_label.trim()).map_err(|e| e.to_string())?;
    services::settings::save_plsql_dev_path(plsql_dev_path.trim()).map_err(|e| e.to_string())?;

    set_client_lib_dir_hint(client_lib_dir.trim());
    let initialized = configure_client_lib_dir(client_lib_dir.trim()).unwrap_or(false);

    Ok(SaveSettingsResponse {
        oracle_client_initialized: initialized,
    })
}

#[tauri::command]
pub fn set_output_folder(folder: String) -> Result<(), String> {
    services::settings::save_output_folder(folder.trim()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_last_query_export_dir(dir: String) -> Result<(), String> {
    services::settings::save_last_query_export_dir(dir.trim()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A `cmd /C …` command that runs without flashing a console window.
#[cfg(target_os = "windows")]
fn hidden_cmd() -> std::process::Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("cmd");
    cmd.creation_flags(CREATE_NO_WINDOW).arg("/C");
    cmd
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    hidden_cmd()
        .args(["start", "", &path])
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "windows"))]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens `path` in VS Code. On Windows `code` is a `.cmd` shim, so it has to go
/// through `cmd`. Blocks briefly to catch a missing `code` command, so call it off
/// the main thread.
pub(crate) fn launch_vscode(path: &std::path::Path) -> Result<(), String> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    if !path.exists() {
        return Err(format!("Folder doesn't exist: {}", path.display()));
    }

    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut cmd = hidden_cmd();
        cmd.arg("code");
        cmd
    };
    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new("code");

    const NOT_FOUND: &str = "Couldn't start VS Code — make sure it's installed and the `code` command is on PATH.";
    // No pipes: VS Code inherits the handles and would keep them open.
    let mut child = cmd
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| NOT_FOUND.to_string())?;

    // `code` hands off to the VS Code process and exits within a moment; a non-zero
    // exit means it couldn't be run. If it's still going, assume it's starting fine.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(NOT_FOUND.to_string()),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn open_in_vscode(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || launch_vscode(std::path::Path::new(&path)))
        .await
        .map_err(|e| e.to_string())?
}
