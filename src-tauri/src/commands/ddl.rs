use std::sync::Arc;

use tauri::State;
use tokio;

use crate::models::SchemaObject;
use crate::services::cancel::begin_task;
use crate::services::ddl_save::{self, DdlSaveArgs, DdlSaveOptions, SaveDdlResult};
use crate::state::AppState;

#[tauri::command]
pub async fn fetch_schema_objects(
    state: State<'_, AppState>,
    server_id: i64,
) -> Result<Vec<SchemaObject>, String> {
    let mut conns = super::selected_connections(&state.catalog, &[server_id])?;
    let conn = conns.remove(0);
    let filter_rules = state
        .filter_rules_svc
        .list_active_rules()
        .map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);

    tokio::task::spawn_blocking(move || {
        let _task = begin_task();
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
    server_id: i64,
    object_name: String,
    object_type: String,
) -> Result<String, String> {
    let mut conns = super::selected_connections(&state.catalog, &[server_id])?;
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

#[tauri::command]
pub fn save_ddl_to_folder(
    state: tauri::State<crate::state::AppState>,
    schema: String,
    object_name: String,
    object_type: String,
    ddl: String,
    description: String,
    db_type: String,
) -> Result<SaveDdlResult, String> {
    let schema_root = crate::services::settings::load_schema_root_folder()
        .ok_or("Schema folder not configured. Please set it in Settings.")?;

    let options = DdlSaveOptions {
        code_folder: crate::services::settings::load_code_folder_name()
            .unwrap_or_else(|| "code".to_string()),
        migration_folder: crate::services::settings::load_migration_folder_name()
            .unwrap_or_else(|| "migration".to_string()),
        storage_mode: crate::services::settings::resolve_storage_mode(&object_type),
        naming_convention: crate::services::settings::load_naming_convention()
            .unwrap_or_else(|| "timestamp".to_string()),
        folder_mode: crate::services::settings::load_migration_folder_mode()
            .unwrap_or_else(|| "year".to_string()),
        version_label: crate::services::settings::load_migration_version_label()
            .unwrap_or_default(),
        extension: crate::services::settings::resolve_ddl_extension(&object_type),
        encoding: crate::services::settings::load_ddl_file_encoding()
            .unwrap_or_else(|| "utf8".to_string()),
    };

    ddl_save::save_ddl_to_folder(
        &state.folder_schema_overrides_svc,
        &schema_root,
        &DdlSaveArgs {
            schema,
            object_name,
            object_type,
            ddl,
            description,
            db_type,
        },
        &options,
    )
}

#[tauri::command]
pub async fn open_schema_in_vscode(
    state: tauri::State<'_, crate::state::AppState>,
    schema: String,
) -> Result<(), String> {
    let schema_root = crate::services::settings::load_schema_root_folder()
        .filter(|root| !root.trim().is_empty())
        .ok_or("Schema folder isn't set. Choose one in Settings first.")?;

    let schema_lower = schema.to_lowercase();
    let schema_dir = state
        .folder_schema_overrides_svc
        .resolve_schema_dir(&schema_lower, &schema_root)
        .map_err(|e| e.to_string())?;
    if !schema_dir.exists() {
        return Err(format!(
            "No folder for {schema} yet ({}). Save some of its DDL first.",
            schema_dir.display()
        ));
    }

    tokio::task::spawn_blocking(move || super::settings::launch_vscode(&schema_dir))
        .await
        .map_err(|e| e.to_string())?
}
