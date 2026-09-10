use std::sync::{Arc, Mutex};

use crate::models::{ServersData, ServerTableDdls};
use crate::repositories::connection_repository::SqliteConnectionRepository;
use crate::repositories::db_repository::DbRepository;
use crate::repositories::dispatch_repository::DispatchRepository;
use crate::repositories::filter_rule_repository::SqliteFilterRuleRepository;
use crate::repositories::folder_schema_override_repository::SqliteSchemaFolderOverrideRepository;
use crate::repositories::history_naming_rule_repository::SqliteHistoryNamingRuleRepository;
use crate::repositories::query_history_repository::SqliteQueryHistoryRepository;
use crate::services::connection_catalog::ConnectionCatalogService;
use crate::services::filter_rules::FilterRuleService;
use crate::services::folder_schema_overrides::SchemaFolderOverrideService;
use crate::services::history_naming_rules::HistoryNamingRuleService;
use crate::services::query_history::QueryHistoryService;
use crate::services::query::QueryService;
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
    pub snapshot: Arc<Mutex<FetchSnapshot>>,
}

impl AppState {
    pub fn new() -> Self {
        let db_repo: Arc<dyn DbRepository> = Arc::new(DispatchRepository::new());
        Self {
            catalog: Arc::new(ConnectionCatalogService::new(Arc::new(
                SqliteConnectionRepository::new(),
            ))),
            diff_svc: Arc::new(SchemaDiffService::new(Arc::clone(&db_repo))),
            query_svc: Arc::new(QueryService::new(Arc::clone(&db_repo))),
            query_history_svc: Arc::new(QueryHistoryService::new(Arc::new(
                SqliteQueryHistoryRepository::new(),
            ))),
            filter_rules_svc: Arc::new(FilterRuleService::new(Arc::new(
                SqliteFilterRuleRepository::new(),
            ))),
            history_naming_rules_svc: Arc::new(HistoryNamingRuleService::new(Arc::new(
                SqliteHistoryNamingRuleRepository::new(),
            ))),
            folder_schema_overrides_svc: Arc::new(SchemaFolderOverrideService::new(Arc::new(
                SqliteSchemaFolderOverrideRepository::new(),
            ))),
            snapshot: Arc::new(Mutex::new(FetchSnapshot::default())),
        }
    }
}
