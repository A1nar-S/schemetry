//! In-memory fakes for unit-testing the MCP gateway without a database, keychain or the
//! shared SQLite file.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};

use crate::models::*;
use crate::repositories::connection_repository::ConnectionRepository;
use crate::repositories::db_repository::{
    DbRepository, QueryOutput, ReadOnlyQuery, ReadOnlyResult,
};
use crate::repositories::filter_rule_repository::FilterRuleRepository;
use crate::repositories::folder_schema_override_repository::SchemaFolderOverrideRepository;
use crate::repositories::mcp_audit_repository::{GENESIS_HASH, McpAuditRepository};
use crate::repositories::mcp_policy_repository::McpPolicyRepository;
use crate::services::connection_catalog::ConnectionCatalogService;
use crate::services::ddl_save::DdlSaveOptions;
use crate::services::filter_rules::FilterRuleService;
use crate::services::folder_schema_overrides::SchemaFolderOverrideService;
use crate::services::mcp_audit::McpAuditService;
use crate::services::mcp_gateway::{GatewaySettings, McpGateway};
use crate::services::mcp_policy::McpPolicyService;
use crate::services::query::QueryService;
use crate::services::schema_diff::SchemaDiffService;

pub fn connection(id: i64, name: &str, db_type: DbType) -> ConnectionRecord {
    ConnectionRecord {
        id,
        name: name.to_string(),
        db_type,
        host: "db.example".to_string(),
        port: 1521,
        service_name: "SVC".to_string(),
        username: "app".to_string(),
        password: "s3cret-password".to_string(),
        group_name: "Tests".to_string(),
        pg_schema: String::new(),
        password_broken: false,
    }
}

pub fn enabled_policy(connection_id: i64) -> McpPolicy {
    McpPolicy {
        connection_id,
        mcp_enabled: true,
        allow_invalid_objects: true,
        ..McpPolicy::default()
    }
}

pub fn allow(table: &str, column: &str) -> McpTableRule {
    McpTableRule {
        action: McpRuleAction::Allow,
        table: table.to_string(),
        column: column.to_string(),
    }
}

pub fn deny(table: &str, column: &str) -> McpTableRule {
    McpTableRule {
        action: McpRuleAction::Deny,
        table: table.to_string(),
        column: column.to_string(),
    }
}

// ── connections ─────────────────────────────────────────────────────────────────

pub struct FakeConnections(pub Mutex<Vec<ConnectionRecord>>);

impl ConnectionRepository for FakeConnections {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn get_all_connections(&self) -> Result<Vec<ConnectionRecord>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn list_connection_summaries(&self) -> Result<Vec<ConnectionRecord>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(|mut c| {
                c.password.clear();
                c
            })
            .collect())
    }
    fn get_connection(&self, id: i64) -> Result<Option<ConnectionRecord>> {
        Ok(self.0.lock().unwrap().iter().find(|c| c.id == id).cloned())
    }
    fn insert_connection(&self, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("not supported"))
    }
    fn update_connection(&self, _: i64, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("not supported"))
    }
    fn delete_connection(&self, _: i64) -> Result<()> {
        Err(anyhow!("not supported"))
    }
    fn delete_all_connections(&self) -> Result<()> {
        Err(anyhow!("not supported"))
    }
    fn upsert_connection(&self, _: &ConnectionRecord) -> Result<()> {
        Err(anyhow!("not supported"))
    }
}

// ── database ────────────────────────────────────────────────────────────────────

/// What a fake database saw for one read-only query.
#[derive(Clone, Debug)]
pub struct SeenQuery {
    pub username: String,
    pub password: String,
    pub sql: String,
    pub binds: Vec<String>,
    pub max_rows: u32,
    pub timeout_secs: u32,
}

#[derive(Default)]
pub struct FakeDb {
    pub objects: Mutex<Vec<SchemaObject>>,
    pub ddls: Mutex<HashMap<String, String>>,
    /// Upper-cased table name → columns.
    pub columns: Mutex<HashMap<String, Vec<String>>>,
    /// Per connection id: tables as the compare sees them.
    pub schemas: Mutex<HashMap<i64, ServerTables>>,
    pub table_ddls: Mutex<TableDdls>,
    pub readonly_result: Mutex<Option<ReadOnlyResult>>,
    pub seen: Mutex<Vec<SeenQuery>>,
    pub invalid_objects: Mutex<Vec<InvalidObject>>,
    /// `"TYPE:UPPER_NAME"` → diagnostics.
    pub object_errors: Mutex<HashMap<String, ObjectErrorList>>,
    /// Counts catalog/DDL/data calls, to prove a denied call never reached the database.
    pub db_calls: Mutex<u32>,
}

impl FakeDb {
    fn touch(&self) {
        *self.db_calls.lock().unwrap() += 1;
    }
}

impl DbRepository for FakeDb {
    fn test_connection(&self, _: &ConnectionRecord) -> Result<()> {
        Ok(())
    }
    fn fetch_single(&self, conn: &ConnectionRecord, _: &[TableFilterRule]) -> Result<ServerTables> {
        self.touch();
        self.schemas
            .lock()
            .unwrap()
            .get(&conn.id)
            .cloned()
            .ok_or_else(|| anyhow!("no schema for {}", conn.id))
    }
    fn fetch_table_ddls(&self, _: &ConnectionRecord, _: &[TableFilterRule]) -> Result<TableDdls> {
        self.touch();
        Ok(self.table_ddls.lock().unwrap().clone())
    }
    fn fetch_table_ddls_for_tables(&self, _: &ConnectionRecord, _: &[String]) -> Result<TableDdls> {
        self.touch();
        Ok(self.table_ddls.lock().unwrap().clone())
    }
    fn fetch_schema_objects(
        &self,
        _: &ConnectionRecord,
        _: &[TableFilterRule],
    ) -> Result<Vec<SchemaObject>> {
        self.touch();
        Ok(self.objects.lock().unwrap().clone())
    }
    fn fetch_object_ddl(
        &self,
        _: &ConnectionRecord,
        name: &str,
        object_type: &str,
    ) -> Result<String> {
        self.touch();
        self.ddls
            .lock()
            .unwrap()
            .get(&format!("{object_type}:{}", name.to_ascii_uppercase()))
            .cloned()
            .ok_or_else(|| anyhow!("DDL not found for {object_type} {name}"))
    }
    fn generate_history_fix(
        &self,
        _: &ConnectionRecord,
        _: &[HistoryNamingRule],
    ) -> Result<HistoryFixResult> {
        Ok(HistoryFixResult::default())
    }
    fn fetch_completion_metadata(&self, _: &ConnectionRecord) -> Result<CompletionMetadata> {
        Ok(CompletionMetadata::default())
    }
    fn run_query(&self, _: &ConnectionRecord, _: &str, _: bool) -> Result<QueryOutput> {
        Err(anyhow!(
            "the MCP must never use the unrestricted query path"
        ))
    }
    fn fetch_table_columns(&self, _: &ConnectionRecord, table: &str) -> Result<Vec<String>> {
        self.touch();
        Ok(self
            .columns
            .lock()
            .unwrap()
            .get(&table.to_ascii_uppercase())
            .cloned()
            .unwrap_or_default())
    }
    fn run_readonly_query(
        &self,
        conn: &ConnectionRecord,
        request: &ReadOnlyQuery,
    ) -> Result<ReadOnlyResult> {
        self.touch();
        self.seen.lock().unwrap().push(SeenQuery {
            username: conn.username.clone(),
            password: conn.password.clone(),
            sql: request.sql.clone(),
            binds: request.binds.clone(),
            max_rows: request.max_rows,
            timeout_secs: request.timeout_secs,
        });
        self.readonly_result
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("no result prepared"))
    }
    fn fetch_invalid_objects(
        &self,
        conn: &ConnectionRecord,
        object_type: Option<&str>,
        max_rows: usize,
    ) -> Result<InvalidObjectList> {
        self.touch();
        if conn.db_type == DbType::Postgres {
            return Ok(InvalidObjectList::not_applicable("not applicable"));
        }
        let mut objects: Vec<InvalidObject> = self
            .invalid_objects
            .lock()
            .unwrap()
            .iter()
            .filter(|o| object_type.is_none_or(|t| o.object_type.eq_ignore_ascii_case(t)))
            .cloned()
            .collect();
        let truncated = objects.len() > max_rows;
        objects.truncate(max_rows);
        Ok(InvalidObjectList {
            applicable: true,
            message: None,
            objects,
            truncated,
        })
    }
    fn fetch_object_errors(
        &self,
        conn: &ConnectionRecord,
        name: &str,
        object_type: &str,
        max_rows: usize,
    ) -> Result<ObjectErrorList> {
        self.touch();
        if conn.db_type == DbType::Postgres {
            return Ok(ObjectErrorList::not_applicable("not applicable"));
        }
        let mut list = self
            .object_errors
            .lock()
            .unwrap()
            .get(&format!("{object_type}:{}", name.to_ascii_uppercase()))
            .cloned()
            .unwrap_or(ObjectErrorList {
                applicable: true,
                message: Some("No such object".into()),
                status: None,
                errors: Vec::new(),
                truncated: false,
            });
        if list.errors.len() > max_rows {
            list.errors.truncate(max_rows);
            list.truncated = true;
        }
        Ok(list)
    }
}

// ── small repositories ──────────────────────────────────────────────────────────

pub struct NoFilterRules;

impl FilterRuleRepository for NoFilterRules {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn list_rules(&self) -> Result<Vec<TableFilterRule>> {
        Ok(Vec::new())
    }
    fn insert_rule(&self, rule: &TableFilterRule) -> Result<TableFilterRule> {
        Ok(rule.clone())
    }
    fn update_rule(&self, _: i64, rule: &TableFilterRule) -> Result<TableFilterRule> {
        Ok(rule.clone())
    }
    fn delete_rule(&self, _: i64) -> Result<()> {
        Ok(())
    }
}

pub struct NoOverrides;

impl SchemaFolderOverrideRepository for NoOverrides {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn list_overrides(&self) -> Result<Vec<SchemaFolderOverride>> {
        Ok(Vec::new())
    }
    fn insert_override(&self, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        Ok(data.clone())
    }
    fn update_override(&self, _: i64, data: &SchemaFolderOverride) -> Result<SchemaFolderOverride> {
        Ok(data.clone())
    }
    fn delete_override(&self, _: i64) -> Result<()> {
        Ok(())
    }
    fn find_for_schema(&self, _: &str) -> Result<Option<SchemaFolderOverride>> {
        Ok(None)
    }
}

#[derive(Default)]
pub struct MemoryPolicies {
    pub policies: Mutex<HashMap<i64, McpPolicy>>,
    pub passwords: Mutex<HashMap<i64, String>>,
    /// Simulates a machine without a keychain: password deletion fails.
    pub keychain_unavailable: AtomicBool,
}

impl McpPolicyRepository for MemoryPolicies {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn get(&self, id: i64) -> Result<Option<McpPolicy>> {
        Ok(self
            .policies
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .map(|mut p| {
                p.mcp_password_set = self.passwords.lock().unwrap().contains_key(&id);
                p
            }))
    }
    fn list_all(&self) -> Result<HashMap<i64, McpPolicy>> {
        Ok(self.policies.lock().unwrap().clone())
    }
    fn upsert(&self, policy: &McpPolicy) -> Result<()> {
        self.policies
            .lock()
            .unwrap()
            .insert(policy.connection_id, policy.clone());
        Ok(())
    }
    fn delete(&self, id: i64) -> Result<()> {
        self.policies.lock().unwrap().remove(&id);
        Ok(())
    }
    fn delete_all(&self) -> Result<()> {
        self.policies.lock().unwrap().clear();
        Ok(())
    }
    fn save_account_password(&self, id: i64, password: &str) -> Result<()> {
        self.passwords
            .lock()
            .unwrap()
            .insert(id, password.to_string());
        Ok(())
    }
    fn load_account_password(&self, id: i64) -> Result<Option<String>> {
        Ok(self.passwords.lock().unwrap().get(&id).cloned())
    }
    fn delete_account_password(&self, id: i64) -> Result<()> {
        if self.keychain_unavailable.load(Ordering::SeqCst) {
            return Err(anyhow!("No default store has been set"));
        }
        self.passwords.lock().unwrap().remove(&id);
        Ok(())
    }
}

/// An audit repository that keeps the chain in memory and can be told to fail.
#[derive(Default)]
pub struct MemoryAudit {
    pub entries: Mutex<Vec<McpAuditEntry>>,
    pub fail_appends: AtomicBool,
}

impl McpAuditRepository for MemoryAudit {
    fn init_db(&self) -> Result<()> {
        Ok(())
    }
    fn append_chained(
        &self,
        build: &mut dyn FnMut(i64, &str) -> McpAuditEntry,
    ) -> Result<McpAuditEntry> {
        if self.fail_appends.load(Ordering::SeqCst) {
            return Err(anyhow!("disk full"));
        }
        let mut entries = self.entries.lock().unwrap();
        let (id, prev) = entries
            .last()
            .map(|e| (e.id + 1, e.hash.clone()))
            .unwrap_or((1, GENESIS_HASH.to_string()));
        let entry = build(id, &prev);
        entries.push(entry.clone());
        Ok(entry)
    }
    fn list(&self, _: &McpAuditFilter) -> Result<Vec<McpAuditEntry>> {
        let mut all = self.entries.lock().unwrap().clone();
        all.reverse();
        Ok(all)
    }
    fn count(&self, _: &McpAuditFilter) -> Result<i64> {
        Ok(self.entries.lock().unwrap().len() as i64)
    }
    fn for_each_in_order(&self, visit: &mut dyn FnMut(McpAuditEntry) -> bool) -> Result<()> {
        for entry in self.entries.lock().unwrap().clone() {
            if !visit(entry) {
                break;
            }
        }
        Ok(())
    }
    fn issued_sequence(&self) -> Result<i64> {
        Ok(self.entries.lock().unwrap().last().map_or(0, |e| e.id))
    }
}

pub struct FakeSettings {
    pub data_tools_disabled: AtomicBool,
    pub schema_root: Mutex<Option<String>>,
    pub options: Mutex<DdlSaveOptions>,
}

impl Default for FakeSettings {
    fn default() -> Self {
        Self {
            data_tools_disabled: AtomicBool::new(false),
            schema_root: Mutex::new(None),
            options: Mutex::new(DdlSaveOptions {
                code_folder: "code".into(),
                migration_folder: "migration".into(),
                storage_mode: "both".into(),
                naming_convention: "timestamp".into(),
                folder_mode: "year".into(),
                version_label: String::new(),
                extension: "sql".into(),
                encoding: "utf8".into(),
            }),
        }
    }
}

impl GatewaySettings for FakeSettings {
    fn data_tools_disabled(&self) -> bool {
        self.data_tools_disabled.load(Ordering::SeqCst)
    }
    fn schema_root(&self) -> Result<String, String> {
        self.schema_root
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "Schema folder not configured.".to_string())
    }
    fn save_options(&self, _: &str) -> DdlSaveOptions {
        self.options.lock().unwrap().clone()
    }
}

/// A gateway wired to fakes, with handles to inspect them.
pub struct Harness {
    pub gateway: McpGateway,
    pub db: Arc<FakeDb>,
    pub audit: Arc<MemoryAudit>,
    pub policies: Arc<MemoryPolicies>,
    pub settings: Arc<FakeSettings>,
}

impl Harness {
    pub fn new(connections: Vec<ConnectionRecord>) -> Self {
        let db = Arc::new(FakeDb::default());
        let audit = Arc::new(MemoryAudit::default());
        let policies = Arc::new(MemoryPolicies::default());
        let settings = Arc::new(FakeSettings::default());
        let db_repo: Arc<dyn DbRepository> = db.clone();
        let gateway = McpGateway::from_parts(
            Arc::new(ConnectionCatalogService::new(Arc::new(FakeConnections(
                Mutex::new(connections),
            )))),
            Arc::new(SchemaDiffService::new(Arc::clone(&db_repo))),
            Arc::new(QueryService::new(db_repo)),
            Arc::new(FilterRuleService::new(Arc::new(NoFilterRules))),
            Arc::new(SchemaFolderOverrideService::new(Arc::new(NoOverrides))),
            Arc::new(McpPolicyService::new(policies.clone())),
            Arc::new(McpAuditService::new(audit.clone()).without_forwarding()),
            settings.clone(),
        );
        Self {
            gateway,
            db,
            audit,
            policies,
            settings,
        }
    }

    pub fn set_policy(&self, policy: McpPolicy) {
        self.policies
            .policies
            .lock()
            .unwrap()
            .insert(policy.connection_id, policy);
    }

    pub fn entries(&self) -> Vec<McpAuditEntry> {
        self.audit.entries.lock().unwrap().clone()
    }

    pub fn db_calls(&self) -> u32 {
        *self.db.db_calls.lock().unwrap()
    }
}
