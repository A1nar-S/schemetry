use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tokio;

use crate::models::SchemaObject;
use crate::state::AppState;

#[tauri::command]
pub async fn fetch_schema_objects(
    state: State<'_, AppState>,
    server_name: String,
) -> Result<Vec<SchemaObject>, String> {
    let mut conns = super::selected_connections(&state.catalog, &[server_name])?;
    let conn = conns.remove(0);
    let filter_rules = state.filter_rules_svc.list_active_rules().map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);

    tokio::task::spawn_blocking(move || {
        diff_svc
            .fetch_schema_objects(&conn, &filter_rules)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn fetch_object_ddl(
    state: State<'_, AppState>,
    server_name: String,
    object_name: String,
    object_type: String,
) -> Result<String, String> {
    let mut conns = super::selected_connections(&state.catalog, &[server_name])?;
    let conn = conns.remove(0);
    let diff_svc = Arc::clone(&state.diff_svc);

    tokio::task::spawn_blocking(move || {
        diff_svc
            .fetch_object_ddl(&conn, &object_name, &object_type)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn encode_content(content: &str, encoding: &str) -> Vec<u8> {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "utf8-bom" => {
            let mut bytes = vec![0xEF, 0xBB, 0xBF];
            bytes.extend_from_slice(content.as_bytes());
            bytes
        }
        "windows-1257" => {
            let (encoded, _, _) = encoding_rs::WINDOWS_1257.encode(content);
            encoded.into_owned()
        }
        _ => content.as_bytes().to_vec(),
    }
}

#[derive(Serialize)]
pub struct SaveDdlResult {
    pub code_path: Option<String>,
    pub migration_path: Option<String>,
}

/// The next Flyway-style version number for a migration folder tree: scans the folder's
/// direct files plus one level of subfolders (matching the `migration/<year>/`,
/// `migration/new/` layout) for existing `V<n>__...` files and returns the highest `n`
/// found, plus one. Self-healing — no persisted counter is needed, so it stays correct
/// even if files were added manually or from another machine.
fn next_migration_version(migration_dir: &std::path::Path) -> u32 {
    fn parse_version(name: &str) -> Option<u32> {
        let rest = name.strip_prefix('V')?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || !rest[digits.len()..].starts_with("__") {
            return None;
        }
        digits.parse().ok()
    }

    fn scan_dir_max(dir: &std::path::Path, max: &mut u32) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(n) = parse_version(name) {
                    *max = (*max).max(n);
                }
            }
        }
    }

    let mut max = 0u32;
    scan_dir_max(migration_dir, &mut max);
    if let Ok(entries) = std::fs::read_dir(migration_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                scan_dir_max(&path, &mut max);
            }
        }
    }
    max + 1
}

#[tauri::command]
pub fn save_ddl_to_folder(
    state: tauri::State<crate::state::AppState>,
    schema: String,
    object_name: String,
    object_type: String,
    ddl: String,
    description: String,
) -> Result<SaveDdlResult, String> {
    use chrono::Local;
    use std::fs;

    let schema_root = crate::services::settings::load_schema_root_folder()
        .ok_or("Schema folder not configured. Please set it in Settings.")?;

    let schema_lower = schema.to_lowercase();

    let schema_dir = state
        .folder_schema_overrides_svc
        .resolve_schema_dir(&schema_lower, &schema_root)
        .map_err(|e| e.to_string())?;

    let global_code_folder = crate::services::settings::load_code_folder_name()
        .unwrap_or_else(|| "code".to_string());
    let code_folder = state
        .folder_schema_overrides_svc
        .resolve_code_folder_name(&schema_lower, &global_code_folder)
        .map_err(|e| e.to_string())?;

    let global_migration_folder = crate::services::settings::load_migration_folder_name()
        .unwrap_or_else(|| "migration".to_string());
    let migration_folder = state
        .folder_schema_overrides_svc
        .resolve_migration_folder_name(&schema_lower, &global_migration_folder)
        .map_err(|e| e.to_string())?;

    let code_dir = schema_dir.join(&code_folder).join(&schema_lower);
    let migration_dir = schema_dir.join(&migration_folder);

    let global_storage_mode = crate::services::settings::resolve_storage_mode(&object_type);
    let storage_mode = state
        .folder_schema_overrides_svc
        .resolve_storage_mode(&schema_lower, &object_type, &global_storage_mode)
        .map_err(|e| e.to_string())?;
    let storage_mode = match storage_mode.trim() {
        m @ ("code" | "migration" | "both") => m.to_string(),
        _ => "both".to_string(),
    };
    let write_code = storage_mode != "migration";
    let write_migration = storage_mode != "code";

    let global_naming = crate::services::settings::load_naming_convention()
        .unwrap_or_else(|| "timestamp".to_string());
    let naming_convention = state
        .folder_schema_overrides_svc
        .resolve_naming_convention(&schema_lower, &global_naming)
        .map_err(|e| e.to_string())?;

    let global_folder_mode = crate::services::settings::load_migration_folder_mode()
        .unwrap_or_else(|| "year".to_string());
    let folder_mode = state
        .folder_schema_overrides_svc
        .resolve_migration_folder_mode(&schema_lower, &global_folder_mode)
        .map_err(|e| e.to_string())?;

    let global_version_label = crate::services::settings::load_migration_version_label().unwrap_or_default();
    let version_label = state
        .folder_schema_overrides_svc
        .resolve_migration_version_label(&schema_lower, &global_version_label)
        .map_err(|e| e.to_string())?;

    let global_ext = crate::services::settings::resolve_ddl_extension(&object_type);
    let ext = state
        .folder_schema_overrides_svc
        .resolve_extension(&schema_lower, &object_type, &global_ext)
        .map_err(|e| e.to_string())?;

    let default_encoding = crate::services::settings::load_ddl_file_encoding()
        .unwrap_or_else(|| "utf8".to_string());
    let encoding = state
        .folder_schema_overrides_svc
        .resolve_encoding(&schema_lower, &default_encoding)
        .map_err(|e| e.to_string())?;

    let mut code_path: Option<String> = None;
    let mut migration_path: Option<String> = None;

    if write_code {
        fs::create_dir_all(&code_dir).map_err(|e| format!("Failed to create code dir: {e}"))?;
        // Flyway naming marks the raw/repeatable file with the "R__" prefix — a code
        // object rebuilt via CREATE OR REPLACE on every save is exactly Flyway's
        // repeatable-migration semantics.
        let code_filename = if naming_convention == "flyway" {
            format!("R__{}.{}", object_name, ext)
        } else {
            format!("{}.{}", object_name, ext)
        };
        let code_file = code_dir.join(code_filename);
        let code_content = encode_content(&ddl, &encoding);
        fs::write(&code_file, &code_content).map_err(|e| format!("Failed to write code file: {e}"))?;
        code_path = Some(code_file.to_string_lossy().into_owned());
    }

    if write_migration {
        fs::create_dir_all(&migration_dir).map_err(|e| format!("Failed to create migration dir: {e}"))?;

        let migration_ddl = crate::repositories::oracle_repository::build_deploy_script(
            &object_type.to_ascii_uppercase(),
            &object_name,
            &ddl,
        );
        let migration_content = encode_content(&migration_ddl, &encoding);

        let now = Local::now();

        // Determine migration subfolder: prefer the configured "primary" folder (the
        // current year, or the manually-set version label) if it already exists, then
        // "new", else fall back to the base migration folder. Folders are never created
        // automatically — they're expected to be set up ahead of time.
        let primary_name = if folder_mode == "version" {
            version_label.trim().to_string()
        } else {
            now.format("%Y").to_string()
        };
        let primary_dir = (!primary_name.is_empty()).then(|| migration_dir.join(&primary_name));
        let new_dir = migration_dir.join("new");
        let actual_migration_dir = match primary_dir {
            Some(dir) if dir.is_dir() => dir,
            _ if new_dir.is_dir() => new_dir,
            _ => migration_dir.clone(),
        };

        let migration_filename = if naming_convention == "flyway" {
            let version = next_migration_version(&migration_dir);
            format!("V{}__{}_{}.{}", version, schema_lower, description, ext)
        } else {
            let timestamp = now.format("%y%m%d_%H%M").to_string();
            format!("{}_{}${}.{}", timestamp, schema_lower, description, ext)
        };
        let migration_file = actual_migration_dir.join(&migration_filename);
        fs::write(&migration_file, &migration_content)
            .map_err(|e| format!("Failed to write migration file: {e}"))?;
        migration_path = Some(migration_file.to_string_lossy().into_owned());
    }

    Ok(SaveDdlResult { code_path, migration_path })
}

#[tauri::command]
pub fn open_schema_in_vscode(
    state: tauri::State<crate::state::AppState>,
    schema: String,
) -> Result<(), String> {
    let schema_root = crate::services::settings::load_schema_root_folder()
        .ok_or("Schema folder not configured. Please set it in Settings.")?;

    let schema_lower = schema.to_lowercase();
    let schema_dir = state
        .folder_schema_overrides_svc
        .resolve_schema_dir(&schema_lower, &schema_root)
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "code", schema_dir.to_str().unwrap_or_default()])
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "windows"))]
    std::process::Command::new("code")
        .arg(&schema_dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
