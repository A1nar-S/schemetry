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
    pub server_id: i64,
    pub server: String,
    pub error: String,
}

#[derive(Serialize)]
pub struct LoadedServer {
    pub id: i64,
    pub name: String,
}

#[derive(Serialize)]
pub struct FetchServersResponse {
    pub loaded_servers: Vec<LoadedServer>,
    pub errors: Vec<ServerError>,
}

#[tauri::command]
pub async fn fetch_servers(
    state: State<'_, AppState>,
    server_ids: Vec<i64>,
) -> Result<FetchServersResponse, String> {
    let selected = super::selected_connections(&state.catalog, &server_ids)?;

    if let Some(first) = selected.first() {
        if selected.iter().any(|c| c.db_type != first.db_type) {
            return Err(
                "Cannot compare servers from different database engines.".to_string(),
            );
        }
    }

    let id_to_name: HashMap<i64, String> = selected.iter().map(|c| (c.id, c.name.clone())).collect();
    let filter_rules = state.filter_rules_svc.list_active_rules().map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);
    let snapshot_lock = Arc::clone(&state.snapshot);

    tokio::task::spawn_blocking(move || {
        let (servers, errors_map) = diff_svc.fetch_from_connections(&selected, &filter_rules);

        let loaded_servers = {
            let mut list: Vec<LoadedServer> = servers
                .keys()
                .filter_map(|id| id_to_name.get(id).map(|name| LoadedServer { id: *id, name: name.clone() }))
                .collect();
            list.sort_by(|a, b| a.name.cmp(&b.name));
            list
        };

        let mut errors: Vec<ServerError> = errors_map
            .into_iter()
            .map(|(id, error)| ServerError {
                server_id: id,
                server: id_to_name.get(&id).cloned().unwrap_or_default(),
                error,
            })
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
    reference_server_id: i64,
    check_comments: bool,
    check_indexes: bool,
) -> Result<Vec<Discrepancy>, String> {
    let server_names: HashMap<i64, String> = state
        .catalog
        .get_all_connections()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| (c.id, c.name))
        .collect();
    let snapshot = state.snapshot.lock().map_err(|_| "Failed to lock server snapshot.")?;
    services::compare::compare_tables_across_servers(
        &snapshot.servers,
        &server_names,
        reference_server_id,
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
    reference_server_id: i64,
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
                    && row.server_id != reference_server_id
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
        let mut conns = super::selected_connections(&state.catalog, &[reference_server_id])?;
        Some(conns.remove(0))
    } else {
        None
    };

    let diff_svc = Arc::clone(&state.diff_svc);
    let snapshot_lock = Arc::clone(&state.snapshot);
    let all_connections = state.catalog.get_all_connections().map_err(|e| e.to_string())?;
    let server_names: HashMap<i64, String> = all_connections
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    let server_dialects: HashMap<i64, services::fix::Dialect> = all_connections
        .into_iter()
        .map(|c| {
            let dialect = match c.db_type {
                crate::models::DbType::Oracle => services::fix::Dialect::Oracle,
                crate::models::DbType::Postgres => services::fix::Dialect::Postgres,
            };
            (c.id, dialect)
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
                map.insert(reference_server_id, ddls);
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
            reference_server_id,
            &server_names,
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
