use std::sync::Arc;
use std::thread;

use tauri::State;
use tokio;

use crate::models::{HistoryNamingRule, ServerHistoryFixResult};
use crate::state::AppState;

#[tauri::command]
pub fn list_history_naming_rules(state: State<AppState>) -> Result<Vec<HistoryNamingRule>, String> {
    state.history_naming_rules_svc.list_rules().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_history_naming_rule(
    state: State<AppState>,
    rule: HistoryNamingRule,
) -> Result<HistoryNamingRule, String> {
    state.history_naming_rules_svc.save_rule(&rule).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_history_naming_rule(state: State<AppState>, id: i64) -> Result<(), String> {
    state.history_naming_rules_svc.delete_rule(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_history_fix(
    state: State<'_, AppState>,
    server_names: Vec<String>,
) -> Result<Vec<ServerHistoryFixResult>, String> {
    let conns = super::selected_connections(&state.catalog, &server_names)?;
    let naming_rules = state.history_naming_rules_svc.list_active_rules().map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);

    tokio::task::spawn_blocking(move || {
        let mut handles = Vec::new();
        for conn in conns {
            let svc = Arc::clone(&diff_svc);
            let rules = naming_rules.clone();
            handles.push(thread::spawn(move || {
                let name = conn.name.clone();
                match svc.generate_history_fix(&conn, &rules) {
                    Ok(res) => ServerHistoryFixResult {
                        server_name: name,
                        issues: res.issues,
                        fix_sql: res.fix_sql,
                        error: None,
                    },
                    Err(e) => ServerHistoryFixResult {
                        server_name: name,
                        issues: vec![],
                        fix_sql: String::new(),
                        error: Some(e.to_string()),
                    },
                }
            }));
        }
        let mut results: Vec<ServerHistoryFixResult> =
            handles.into_iter().filter_map(|h| h.join().ok()).collect();
        results.sort_by(|a, b| a.server_name.cmp(&b.server_name));
        Ok(results)
    })
    .await
    .map_err(|e| e.to_string())?
}
