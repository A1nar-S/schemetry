pub mod commands;
pub mod logging;
pub mod models;
pub mod repositories;
pub mod services;
pub mod state;
pub mod utils;

use anyhow::{Context, Result};
use std::ffi::OsString;

use crate::repositories::oracle_repository::{configure_client_lib_dir, set_client_lib_dir_hint};
use crate::state::{AppServices, AppState};

pub fn init(db_override: Option<OsString>) -> Result<AppServices> {
    utils::paths::configure_db_path(db_override)
        .context("Failed to configure SQLite database path")?;

    services::settings::init_db().context("Failed to initialize settings table")?;
    let services = AppServices::new();
    services
        .catalog
        .init_db()
        .context("Failed to initialize connections table")?;
    services
        .query_history_svc
        .init_db()
        .context("Failed to initialize query history table")?;
    services
        .filter_rules_svc
        .init_db()
        .context("Failed to initialize table filter rules")?;
    services
        .history_naming_rules_svc
        .init_db()
        .context("Failed to initialize history naming rules")?;
    services
        .folder_schema_overrides_svc
        .init_db()
        .context("Failed to initialize schema folder overrides")?;

    if let Some(dir) = services::settings::load_client_lib_dir() {
        set_client_lib_dir_hint(&dir);
        if let Err(err) = configure_client_lib_dir(&dir) {
            eprintln!("Oracle client init warning ({dir}): {err}");
        }
    }

    Ok(services)
}

pub fn run(db_override: Option<OsString>) -> anyhow::Result<()> {
    logging::install_panic_hook();
    let services = init(db_override)?;

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::from_services(services))
        .invoke_handler(tauri::generate_handler![
            commands::connections::get_connections,
            commands::connections::save_connection,
            commands::connections::delete_connection,
            commands::connections::delete_all_connections,
            commands::connections::test_connection,
            commands::connections::import_connections,
            commands::connections::export_connections,
            commands::connections::open_in_plsql_developer,
            commands::compare::fetch_servers,
            commands::compare::compare_discrepancies,
            commands::compare::generate_fix_script,
            commands::compare::export_compare_report,
            commands::ddl::fetch_schema_objects,
            commands::ddl::fetch_object_ddl,
            commands::ddl::save_ddl_to_folder,
            commands::history_fix::generate_history_fix,
            commands::history_fix::list_history_naming_rules,
            commands::history_fix::save_history_naming_rule,
            commands::history_fix::delete_history_naming_rule,
            commands::query::run_query,
            commands::cancel_task,
            commands::query::fetch_completion_metadata,
            commands::query::fetch_lob_content,
            commands::query::save_blob_to_file,
            commands::query::get_query_history,
            commands::query::delete_query_history_item,
            commands::query::pin_query_history_item,
            commands::query::set_query_favorite,
            commands::query::reorder_favorites,
            commands::query::clear_query_history,
            commands::query::export_query_results,
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::set_output_folder,
            commands::settings::set_last_query_export_dir,
            commands::settings::open_folder,
            commands::settings::open_file,
            commands::settings::open_in_vscode,
            commands::ddl::open_schema_in_vscode,
            commands::filter_rules::list_table_filter_rules,
            commands::filter_rules::save_table_filter_rule,
            commands::filter_rules::delete_table_filter_rule,
            commands::folder_schema_overrides::list_folder_schema_overrides,
            commands::folder_schema_overrides::save_folder_schema_override,
            commands::folder_schema_overrides::delete_folder_schema_override,
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/bootstrap.rs"]
mod tests;
