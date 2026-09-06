use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tokio;

use crate::models::{Discrepancy};
use crate::services;
use crate::state::AppState;

#[derive(Serialize)]
pub struct ServerError {
    pub server: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct FetchServersResponse {
    pub loaded_servers: Vec<String>,
    pub errors: Vec<ServerError>,
}

#[tauri::command]
pub async fn fetch_servers(
    state: State<'_, AppState>,
    server_names: Vec<String>,
) -> Result<FetchServersResponse, String> {
    let selected = super::selected_connections(&state.catalog, &server_names)?;
    let filter_rules = state.filter_rules_svc.list_active_rules().map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);
    let snapshot_lock = Arc::clone(&state.snapshot);

    tokio::task::spawn_blocking(move || {
        let (servers, errors_map) = diff_svc.fetch_from_connections(&selected, &filter_rules);

        let loaded_servers = {
            let mut names: Vec<String> = servers.keys().cloned().collect();
            names.sort_unstable();
            names
        };

        let mut errors: Vec<ServerError> = errors_map
            .into_iter()
            .map(|(server, error)| ServerError { server, error })
            .collect();
        errors.sort_by(|a, b| a.server.cmp(&b.server));

        let mut snapshot = snapshot_lock.lock().map_err(|_| "Failed to lock server snapshot.".to_string())?;
        snapshot.servers = servers;
        snapshot.server_table_ddls = HashMap::new();

        Ok(FetchServersResponse { loaded_servers, errors })
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn compare_discrepancies(
    state: State<AppState>,
    reference_server: String,
    check_comments: bool,
    check_indexes: bool,
) -> Result<Vec<Discrepancy>, String> {
    let snapshot = state.snapshot.lock().map_err(|_| "Failed to lock server snapshot.")?;
    services::compare::compare_tables_across_servers(
        &snapshot.servers,
        &reference_server,
        check_comments,
        check_indexes,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn generate_fix_script(
    state: State<'_, AppState>,
    discrepancies: Vec<Discrepancy>,
    selected_ids: Vec<usize>,
    reference_server: String,
) -> Result<services::fix::FixScriptResult, String> {
    let selected_set: HashSet<usize> = selected_ids.into_iter().collect();

    // Collect the distinct table names we need DDLs for: only MissingTable
    // discrepancies where the table exists in the reference server.
    let missing_table_names: Vec<String> = {
        let mut set = HashSet::new();
        for id in &selected_set {
            if let Some(row) = discrepancies.get(*id) {
                if row.difference.eq_ignore_ascii_case("MISSING")
                    && row.column_name.trim().is_empty()
                    && !row.server_name.eq_ignore_ascii_case(&reference_server)
                {
                    set.insert(row.table_name.trim().to_ascii_uppercase());
                }
            }
        }
        set.into_iter().collect()
    };

    // Resolve the reference server connection on the calling thread before moving
    // into spawn_blocking.
    let ref_conn = if !missing_table_names.is_empty() {
        let mut conns = super::selected_connections(&state.catalog, &[reference_server.clone()])?;
        Some(conns.remove(0))
    } else {
        None
    };

    let diff_svc = Arc::clone(&state.diff_svc);
    let snapshot_lock = Arc::clone(&state.snapshot);
    let server_dialects: HashMap<String, services::fix::Dialect> = state
        .catalog
        .get_all_connections()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| {
            let dialect = match c.db_type {
                crate::models::DbType::Oracle => services::fix::Dialect::Oracle,
                crate::models::DbType::Postgres => services::fix::Dialect::Postgres,
            };
            (c.name, dialect)
        })
        .collect();

    tokio::task::spawn_blocking(move || {
        // Lazily fetch only the DDLs actually needed for this fix generation.
        let server_table_ddls = match ref_conn {
            Some(conn) => {
                let ddls = diff_svc
                    .fetch_table_ddls_for_tables(&conn, &missing_table_names)
                    .unwrap_or_default();
                let mut map = HashMap::new();
                map.insert(reference_server.clone(), ddls);
                map
            }
            None => HashMap::new(),
        };

        let snapshot = snapshot_lock.lock().map_err(|_| "Failed to lock server snapshot.".to_string())?;

        services::fix::generate_fix_script(
            &discrepancies,
            &selected_set,
            &snapshot.servers,
            &server_table_ddls,
            &reference_server,
            &server_dialects,
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn export_compare_report(
    discrepancies: Vec<Discrepancy>,
    output_folder: String,
) -> Result<(String, String), String> {
    let (csv_path, xlsx_path) =
        services::compare_export::save_discrepancy_reports(&discrepancies, Path::new(&output_folder))
            .map_err(|e| e.to_string())?;
    Ok((
        csv_path.to_string_lossy().into_owned(),
        xlsx_path.to_string_lossy().into_owned(),
    ))
}
