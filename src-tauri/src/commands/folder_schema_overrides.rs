use tauri::State;

use crate::models::SchemaFolderOverride;
use crate::state::AppState;

#[tauri::command]
pub fn list_folder_schema_overrides(state: State<AppState>) -> Result<Vec<SchemaFolderOverride>, String> {
    state.folder_schema_overrides_svc.list_overrides().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_folder_schema_override(
    state: State<AppState>,
    data: SchemaFolderOverride,
) -> Result<SchemaFolderOverride, String> {
    if data.schema_name.trim().is_empty() {
        return Err("Schema name is required.".to_string());
    }
    if data.folder_path.trim().is_empty()
        && data.encoding.trim().is_empty()
        && data.extensions.trim().is_empty()
        && data.storage_modes.trim().is_empty()
        && data.naming_convention.trim().is_empty()
        && data.code_folder_name.trim().is_empty()
        && data.migration_folder_name.trim().is_empty()
        && data.migration_folder_mode.trim().is_empty()
        && data.migration_version_label.trim().is_empty()
    {
        return Err("Set at least one field to override.".to_string());
    }
    let data = SchemaFolderOverride {
        schema_name: data.schema_name.trim().to_lowercase(),
        folder_path: data.folder_path.trim().to_string(),
        encoding: data.encoding.trim().to_string(),
        extensions: data.extensions.trim().to_string(),
        storage_modes: data.storage_modes.trim().to_string(),
        naming_convention: data.naming_convention.trim().to_string(),
        code_folder_name: data.code_folder_name.trim().to_string(),
        migration_folder_name: data.migration_folder_name.trim().to_string(),
        migration_folder_mode: data.migration_folder_mode.trim().to_string(),
        migration_version_label: data.migration_version_label.trim().to_string(),
        ..data
    };
    state.folder_schema_overrides_svc.save_override(&data).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_folder_schema_override(state: State<AppState>, id: i64) -> Result<(), String> {
    state.folder_schema_overrides_svc.delete_override(id).map_err(|e| e.to_string())
}
