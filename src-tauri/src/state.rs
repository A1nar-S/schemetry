use std::sync::{Arc, Mutex};

use schemetry_core::state::AppServices;

use crate::models::{ServerTableDdls, ServersData};
use crate::services::connection_catalog::ConnectionCatalogService;
use crate::services::filter_rules::FilterRuleService;
use crate::services::folder_schema_overrides::SchemaFolderOverrideService;
use crate::services::history_naming_rules::HistoryNamingRuleService;
use crate::services::mcp_apply_approval::McpApprovalService;
use crate::services::mcp_audit::McpAuditService;
use crate::services::mcp_policy::McpPolicyService;
use crate::services::query::QueryService;
use crate::services::query_history::QueryHistoryService;
use crate::services::schema_diff::SchemaDiffService;

#[derive(Default)]
pub struct FetchSnapshot {
    pub servers: ServersData,
    pub server_table_ddls: ServerTableDdls,
}

pub struct AppState {
    pub catalog: Arc<ConnectionCatalogService>,
    pub diff_svc: Arc<SchemaDiffService>,
    pub query_svc: Arc<QueryService>,
    pub query_history_svc: Arc<QueryHistoryService>,
    pub filter_rules_svc: Arc<FilterRuleService>,
    pub history_naming_rules_svc: Arc<HistoryNamingRuleService>,
    pub folder_schema_overrides_svc: Arc<SchemaFolderOverrideService>,
    pub mcp_policy_svc: Arc<McpPolicyService>,
    pub mcp_audit_svc: Arc<McpAuditService>,
    pub mcp_approval_svc: Arc<McpApprovalService>,
    pub snapshot: Arc<Mutex<FetchSnapshot>>,
}

impl AppState {
    pub fn from_services(services: AppServices) -> Self {
        Self {
            catalog: services.catalog,
            diff_svc: services.diff_svc,
            query_svc: services.query_svc,
            query_history_svc: services.query_history_svc,
            filter_rules_svc: services.filter_rules_svc,
            history_naming_rules_svc: services.history_naming_rules_svc,
            folder_schema_overrides_svc: services.folder_schema_overrides_svc,
            mcp_policy_svc: services.mcp_policy_svc,
            mcp_audit_svc: services.mcp_audit_svc,
            mcp_approval_svc: services.mcp_approval_svc,
            snapshot: Arc::new(Mutex::new(FetchSnapshot::default())),
        }
    }
}
