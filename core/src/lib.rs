//! Everything Schemetry's front ends share: models, repositories, services, the SQLite
//! bootstrap and the MCP policy/audit/gateway logic. It has no GUI dependencies, so the
//! headless `schemetry-mcp` server builds without WebView/GTK libraries.

pub mod logging;
pub mod models;
pub mod repositories;
pub mod services;
pub mod state;
#[cfg(test)]
pub(crate) mod test_support;
pub mod utils;

use anyhow::{Context, Result};
use std::ffi::OsString;

use crate::repositories::oracle_repository::{configure_client_lib_dir, set_client_lib_dir_hint};
use crate::state::AppServices;

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
    services
        .mcp_policy_svc
        .init_db()
        .context("Failed to initialize MCP policies")?;
    services
        .mcp_audit_svc
        .init_db()
        .context("Failed to initialize the MCP audit log")?;
    services
        .mcp_approval_svc
        .init_db()
        .context("Failed to initialize the MCP deploy approvals")?;

    if let Some(dir) = services::settings::load_client_lib_dir() {
        set_client_lib_dir_hint(&dir);
        if let Err(err) = configure_client_lib_dir(&dir) {
            eprintln!("Oracle client init warning ({dir}): {err}");
        }
    }

    Ok(services)
}

#[cfg(test)]
#[path = "tests/bootstrap.rs"]
mod tests;
