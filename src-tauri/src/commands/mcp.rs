//! Tauri commands for the MCP server's configuration and (read-only) audit log. The
//! audit commands only read: there is deliberately no way to edit or purge entries.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::models::{
    DbType, McpAuditEntry, McpAuditFilter, McpAuditVerification, McpPolicy, SchemaObject,
};
use crate::services::mcp_audit_sink::{self, SinkConfig};
use crate::services::mcp_policy::{AccountPassword, McpTool};
use crate::services::settings;
use crate::state::AppState;

#[tauri::command]
pub fn list_mcp_policies(state: State<AppState>) -> Result<Vec<McpPolicy>, String> {
    let stored = state.mcp_policy_svc.list_all().map_err(|e| e.to_string())?;
    let connections = state
        .catalog
        .list_connection_summaries()
        .map_err(|e| e.to_string())?;
    Ok(connections
        .into_iter()
        .map(|c| {
            stored.get(&c.id).cloned().unwrap_or(McpPolicy {
                connection_id: c.id,
                ..McpPolicy::default()
            })
        })
        .collect())
}

#[tauri::command]
pub fn get_mcp_policy(state: State<AppState>, connection_id: i64) -> Result<McpPolicy, String> {
    state
        .mcp_policy_svc
        .get(connection_id)
        .map_err(|e| e.to_string())
}

/// `account_password` replaces the dedicated account's keychain password when given;
/// `clear_account_password` removes it. Neither is ever returned.
#[tauri::command]
pub fn save_mcp_policy(
    state: State<AppState>,
    policy: McpPolicy,
    account_password: Option<String>,
    clear_account_password: bool,
) -> Result<McpPolicy, String> {
    let known = state
        .catalog
        .list_connection_summaries()
        .map_err(|e| e.to_string())?
        .iter()
        .any(|c| c.id == policy.connection_id);
    if !known {
        return Err("That connection no longer exists.".to_string());
    }
    let password = if clear_account_password {
        AccountPassword::Clear
    } else {
        match account_password.filter(|p| !p.is_empty()) {
            Some(value) => AccountPassword::Set(value),
            None => AccountPassword::Keep,
        }
    };
    state
        .mcp_policy_svc
        .save(policy, password)
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct McpToolStatus {
    pub tool: String,
    pub allowed: bool,
    pub reason: String,
}

#[derive(Serialize)]
pub struct McpPreviewObject {
    pub name: String,
    pub object_type: String,
    /// `allowed`, `restricted` (some columns hidden), `denied`, or `n/a` for non-table objects.
    pub data_access: String,
}

#[derive(Serialize)]
pub struct McpPreview {
    pub tools: Vec<McpToolStatus>,
    pub objects: Vec<McpPreviewObject>,
    pub hidden_objects: usize,
    pub data_tools_disabled: bool,
    pub objects_error: Option<String>,
}

/// "Test as MCP": what an agent would see for this connection under the saved policy.
#[tauri::command]
pub async fn mcp_preview(
    state: State<'_, AppState>,
    connection_id: i64,
) -> Result<McpPreview, String> {
    let policy = state
        .mcp_policy_svc
        .get(connection_id)
        .map_err(|e| e.to_string())?;
    let data_tools_disabled = settings::load_mcp_data_tools_disabled();

    let tools: Vec<McpToolStatus> = McpTool::ALL
        .iter()
        .map(|tool| {
            let verdict = if tool.is_data_tool() && data_tools_disabled && policy.mcp_enabled {
                Err("Data tools are switched off globally in Settings.".to_string())
            } else {
                policy.check_tool(*tool)
            };
            McpToolStatus {
                tool: tool.name().to_string(),
                allowed: verdict.is_ok(),
                reason: verdict.err().unwrap_or_default(),
            }
        })
        .collect();

    let mut preview = McpPreview {
        tools,
        objects: Vec::new(),
        hidden_objects: 0,
        data_tools_disabled,
        objects_error: None,
    };
    if !policy.mcp_enabled || !policy.allow_schema_list {
        return Ok(preview);
    }

    let conn = super::selected_connections(&state.catalog, &[connection_id])?.remove(0);
    let rules = state
        .filter_rules_svc
        .list_active_rules()
        .map_err(|e| e.to_string())?;
    let diff_svc = Arc::clone(&state.diff_svc);
    let db_type: DbType = conn.db_type;

    let fetched: Result<Vec<SchemaObject>, String> = tokio::task::spawn_blocking(move || {
        diff_svc
            .fetch_schema_objects(&conn, &rules)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?;

    match fetched {
        Ok(objects) => {
            for object in objects {
                if policy.object_denied(&object.name, &object.object_type) {
                    preview.hidden_objects += 1;
                    continue;
                }
                let relation = matches!(
                    object.object_type.as_str(),
                    "TABLE" | "VIEW" | "MATERIALIZED VIEW"
                );
                let data_access = if !relation {
                    "n/a"
                } else if !policy.table_allowed(&object.name, db_type) {
                    "denied"
                } else if policy.has_column_rules_for(&object.name, db_type) {
                    "restricted"
                } else {
                    "allowed"
                };
                preview.objects.push(McpPreviewObject {
                    name: object.name,
                    object_type: object.object_type,
                    data_access: data_access.to_string(),
                });
            }
        }
        Err(error) => preview.objects_error = Some(error),
    }
    Ok(preview)
}

#[derive(Serialize, Deserialize)]
pub struct McpSettings {
    pub data_tools_disabled: bool,
    /// `""`, `syslog`, `eventlog` or `http`.
    pub audit_sink_kind: String,
    pub audit_sink_target: String,
}

#[tauri::command]
pub fn get_mcp_settings() -> McpSettings {
    let (kind, target) = settings::load_mcp_audit_sink();
    McpSettings {
        data_tools_disabled: settings::load_mcp_data_tools_disabled(),
        audit_sink_kind: kind,
        audit_sink_target: target,
    }
}

#[tauri::command]
pub fn save_mcp_settings(payload: McpSettings) -> Result<(), String> {
    settings::save_mcp_data_tools_disabled(payload.data_tools_disabled)
        .map_err(|e| e.to_string())?;
    settings::save_mcp_audit_sink(&payload.audit_sink_kind, &payload.audit_sink_target)
        .map_err(|e| e.to_string())
}

/// Sends a synthetic entry to the given sink so the user can check it is reachable. The
/// entry is not written to the audit log.
#[tauri::command]
pub async fn test_mcp_audit_sink(kind: String, target: String) -> Result<(), String> {
    let config = mcp_audit_sink::parse_config(&kind, &target);
    if config == SinkConfig::None {
        return Err("Choose a sink and fill in its target first.".to_string());
    }
    let entry = McpAuditEntry {
        id: 0,
        timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        tool: "sink_test".to_string(),
        connection_id: None,
        connection_name: String::new(),
        arguments: "{}".to_string(),
        outcome: crate::models::McpOutcome::Allowed,
        reason: "Test message from Schemetry settings".to_string(),
        row_count: None,
        bytes: 0,
        duration_ms: 0,
        client_name: "schemetry-gui".to_string(),
        prev_hash: String::new(),
        hash: String::new(),
    };
    tokio::task::spawn_blocking(move || {
        mcp_audit_sink::deliver(&config, &entry).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct McpAuditPage {
    pub entries: Vec<McpAuditEntry>,
    /// Entries matching the filter, ignoring paging.
    pub total: i64,
}

#[tauri::command]
pub fn mcp_audit_list(
    state: State<AppState>,
    filter: McpAuditFilter,
) -> Result<McpAuditPage, String> {
    Ok(McpAuditPage {
        entries: state
            .mcp_audit_svc
            .list(&filter)
            .map_err(|e| e.to_string())?,
        total: state
            .mcp_audit_svc
            .count(&filter)
            .map_err(|e| e.to_string())?,
    })
}

/// Walks the hash chain; the UI shows a warning banner unless `ok`.
#[tauri::command]
pub async fn mcp_audit_verify(state: State<'_, AppState>) -> Result<McpAuditVerification, String> {
    let audit = Arc::clone(&state.mcp_audit_svc);
    tokio::task::spawn_blocking(move || audit.verify().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Writes every entry matching `filter` to a timestamped CSV in `output_folder` and
/// returns its path.
#[tauri::command]
pub async fn mcp_audit_export_csv(
    state: State<'_, AppState>,
    filter: McpAuditFilter,
    output_folder: String,
) -> Result<String, String> {
    let audit = Arc::clone(&state.mcp_audit_svc);
    tokio::task::spawn_blocking(move || {
        let csv = audit.export_csv(&filter).map_err(|e| e.to_string())?;
        let folder = Path::new(&output_folder);
        std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
        let name = format!(
            "mcp_audit_{}.csv",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        let path = folder.join(name);
        std::fs::write(&path, csv).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}
