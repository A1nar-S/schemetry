use tauri::State;

use crate::models::TableFilterRule;
use crate::state::AppState;

#[tauri::command]
pub fn list_table_filter_rules(state: State<AppState>) -> Result<Vec<TableFilterRule>, String> {
    state.filter_rules_svc.list_rules().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_table_filter_rule(
    state: State<AppState>,
    rule: TableFilterRule,
) -> Result<TableFilterRule, String> {
    state.filter_rules_svc.save_rule(&rule).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_table_filter_rule(state: State<AppState>, id: i64) -> Result<(), String> {
    state.filter_rules_svc.delete_rule(id).map_err(|e| e.to_string())
}
