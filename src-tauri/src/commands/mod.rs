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

/// Resolves connections by their unique `id`, not `name` — names may repeat across
/// groups/engines, so id is the only identifier that unambiguously picks one connection.
pub(crate) fn selected_connections(
    catalog: &ConnectionCatalogService,
    server_ids: &[i64],
) -> Result<Vec<ConnectionRecord>, String> {
    let all = catalog.get_all_connections().map_err(|e| e.to_string())?;
    if server_ids.is_empty() {
        return Ok(all);
    }

    let selected: HashSet<i64> = server_ids.iter().copied().collect();

    let picked: Vec<ConnectionRecord> = all
        .into_iter()
        .filter(|conn| selected.contains(&conn.id))
        .collect();

    if picked.is_empty() {
        return Err("No matching server connections were selected.".to_string());
    }

    Ok(picked)
}
