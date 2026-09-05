use serde::Deserialize;
use tauri::State;

use crate::models::ConnectionRecord;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct SaveConnectionRequest {
    pub editing_id: Option<i64>,
    pub connection: ConnectionRecord,
}

#[tauri::command]
pub fn get_connections(state: State<AppState>) -> Result<Vec<ConnectionRecord>, String> {
    state.catalog.get_all_connections().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_connection(
    state: State<AppState>,
    payload: SaveConnectionRequest,
) -> Result<(), String> {
    match payload.editing_id {
        None => state.catalog.insert_connection(&payload.connection).map_err(|e| e.to_string()),
        Some(id) => state.catalog.update_connection(id, &payload.connection).map_err(|e| e.to_string()),
    }
}

#[tauri::command]
pub fn delete_connection(state: State<AppState>, id: i64) -> Result<(), String> {
    state.catalog.delete_connection(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_all_connections(state: State<AppState>) -> Result<(), String> {
    state.catalog.delete_all_connections().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn test_connection(
    state: State<AppState>,
    connection: ConnectionRecord,
) -> Result<(), String> {
    state.diff_svc.test_connection(&connection).map_err(|e| e.to_string())
}

/// Import connections from a JSON array string. Existing connections with the same name
/// are overwritten. Returns the number of records imported.
#[tauri::command]
pub fn import_connections(state: State<AppState>, json: String) -> Result<usize, String> {
    state.catalog.import_from_json(&json).map_err(|e| e.to_string())
}

/// Export-only view of a connection: mirrors `ConnectionRecord` but omits the local
/// SQLite `id`, which is meaningless outside this database (import matches by name/group).
#[derive(serde::Serialize)]
struct ExportedConnection<'a> {
    name: &'a str,
    host: &'a str,
    port: u16,
    service_name: &'a str,
    username: &'a str,
    password: &'a str,
    group_name: &'a str,
}

/// Export all connections as a JSON file at the given path (passwords included, `id` omitted).
#[tauri::command]
pub fn export_connections(state: State<AppState>, path: String) -> Result<usize, String> {
    let conns = state.catalog.get_all_connections().map_err(|e| e.to_string())?;
    let exported: Vec<ExportedConnection> = conns
        .iter()
        .map(|c| ExportedConnection {
            name: &c.name,
            host: &c.host,
            port: c.port,
            service_name: &c.service_name,
            username: &c.username,
            password: &c.password,
            group_name: &c.group_name,
        })
        .collect();
    let json = serde_json::to_string_pretty(&exported).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    Ok(conns.len())
}

/// Launch PL/SQL Developer, logging into the given connection via its command-line
/// `userid` parameter. The executable path comes from Settings, falling back to
/// auto-detection under Program Files on Windows.
#[tauri::command]
pub fn open_in_plsql_developer(connection: ConnectionRecord) -> Result<(), String> {
    use std::path::Path;

    let exe = crate::services::settings::load_plsql_dev_path()
        .filter(|p| Path::new(p).is_file())
        .or_else(find_plsql_dev_exe)
        .ok_or_else(|| {
            "PL/SQL Developer executable not found. Set its path in Settings.".to_string()
        })?;

    // Mirror the EZConnect descriptor the app uses for its own Oracle connections.
    let userid = format!(
        "userid={}/{}@//{}:{}/{}",
        connection.username,
        connection.password,
        connection.host,
        connection.port,
        connection.service_name,
    );

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg(userid);

    // Schemetry (esp. under `npm run tauri dev`) inherits the dev shell's environment, which
    // carries its 64-bit Oracle client on PATH; a 32-bit PL/SQL Developer inheriting that
    // hits ORA-12557. Launch it with the clean logon environment instead — exactly what a
    // Start-menu/shortcut launch gets — so it uses its own Oracle client.
    if let Some(env) = logon_environment() {
        cmd.env_clear();
        cmd.envs(env);
    }

    cmd.spawn()
        .map_err(|e| format!("Failed to launch PL/SQL Developer: {e}"))?;
    Ok(())
}

/// The current user's logon environment (merged HKLM + HKCU, `%var%`-expanded) — the same
/// block Explorer hands a process launched from a shortcut. Returns `None` on any failure.
#[cfg(windows)]
fn logon_environment() -> Option<Vec<(String, String)>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE};
    use windows_sys::Win32::Security::TOKEN_QUERY;
    use windows_sys::Win32::System::Environment::{
        CreateEnvironmentBlock, DestroyEnvironmentBlock,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }

        let mut block: *mut core::ffi::c_void = std::ptr::null_mut();
        let created = CreateEnvironmentBlock(&mut block, token, FALSE);
        CloseHandle(token);
        if created == 0 || block.is_null() {
            return None;
        }

        // The block is a double-NUL-terminated list of UTF-16 `NAME=VALUE` strings.
        let mut pairs = Vec::new();
        let mut p = block as *const u16;
        loop {
            // length of the current NUL-terminated UTF-16 string
            let mut len = 0usize;
            while *p.add(len) != 0 {
                len += 1;
            }
            if len == 0 {
                break; // empty string → end of block
            }
            let slice = std::slice::from_raw_parts(p, len);
            let entry = std::ffi::OsString::from_wide(slice);
            if let Some(text) = entry.to_str() {
                if let Some((k, v)) = text.split_once('=') {
                    // Skip the leading "=C:" drive-letter pseudo-vars Windows includes.
                    if !k.is_empty() {
                        pairs.push((k.to_string(), v.to_string()));
                    }
                }
            }
            p = p.add(len + 1);
        }

        DestroyEnvironmentBlock(block);
        if pairs.is_empty() {
            None
        } else {
            Some(pairs)
        }
    }
}

#[cfg(not(windows))]
fn logon_environment() -> Option<Vec<(String, String)>> {
    None
}

/// Look for a `plsqldev.exe` under a `PLSQL Developer*` folder in the standard
/// Program Files locations.
#[cfg(target_os = "windows")]
fn find_plsql_dev_exe() -> Option<String> {
    use std::path::PathBuf;

    let roots = [
        std::env::var("ProgramW6432").ok(),
        std::env::var("ProgramFiles").ok(),
        std::env::var("ProgramFiles(x86)").ok(),
    ];
    for root in roots.into_iter().flatten() {
        let entries = match std::fs::read_dir(PathBuf::from(&root)) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().to_lowercase().starts_with("plsql developer") {
                let exe = entry.path().join("plsqldev.exe");
                if exe.is_file() {
                    return Some(exe.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn find_plsql_dev_exe() -> Option<String> {
    None
}
