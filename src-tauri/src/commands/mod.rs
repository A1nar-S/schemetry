pub mod compare;
pub mod connections;
pub mod ddl;
pub mod filter_rules;
pub mod folder_schema_overrides;
pub mod history_fix;
pub mod query;
pub mod settings;

use std::collections::HashSet;

use crate::models::ConnectionRecord;
use crate::services::connection_catalog::ConnectionCatalogService;

pub(crate) fn selected_connections(
    catalog: &ConnectionCatalogService,
    server_names: &[String],
) -> Result<Vec<ConnectionRecord>, String> {
    let all = catalog.get_all_connections().map_err(|e| e.to_string())?;
    if server_names.is_empty() {
        return Ok(all);
    }

    let selected: HashSet<String> = server_names
        .iter()
        .map(|s| s.trim().to_ascii_uppercase())
        .collect();

    let picked: Vec<ConnectionRecord> = all
        .into_iter()
        .filter(|conn| selected.contains(&conn.name.to_ascii_uppercase()))
        .collect();

    if picked.is_empty() {
        return Err("No matching server connections were selected.".to_string());
    }

    Ok(picked)
}
