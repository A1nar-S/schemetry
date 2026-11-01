export type SchemaObject = {
  name: string;
  object_type: string;
};

export type CompletionMetadata = {
  schema: string;
  relations: { name: string; kind: string; columns: { name: string; data_type: string }[] }[];
  objects: SchemaObject[];
};

export type DbType = 'oracle' | 'postgres';

export type ConnectionRecord = {
  id: number;
  name: string;
  db_type: DbType;
  host: string;
  port: number;
  /** Oracle: the service name (e.g. ORCL). Postgres: the database name. */
  service_name: string;
  username: string;
  password: string;
  group_name: string;
  /** Postgres only: schema to introspect/target within `service_name`'s database. Empty means `public`. */
  pg_schema?: string;
  password_broken?: boolean;
};

export type QueryServerResult = {
  server_id: number;
  server_name: string;
  // Key for this result's LOB cells.
  result_id: number;
  columns: string[];
  column_types: string[];
  rows: (string | null)[][];
  error: string | null;
  duration_ms?: number;
};

export type LobContent = {
  kind: 'text' | 'binary';
  text: string | null;
  mime: string | null;
  base64: string | null;
  size: number;
  truncated: boolean;
};

export type QueryHistoryEntry = {
  id: number;
  sql_text: string;
  pinned: boolean;
  favorite: boolean;
  description: string;
  created_at: string; // UTC, "YYYY-MM-DD HH:MM:SS"
};

export type Discrepancy = {
  difference: string;
  element: string;
  table_name: string;
  column_name: string;
  server_id: number;
  server_name: string;
  details: string;
};

export type LoadedServer = {
  id: number;
  name: string;
};

export type FetchServersResponse = {
  loaded_servers: LoadedServer[];
  errors: { server_id: number; server: string; error: string }[];
};

export type FixScriptResult = {
  script: string;
  generated_count: number;
  skipped_count: number;
};

export type AppSettings = {
  output_folder: string;
  client_lib_dir: string;
  last_query_export_dir: string;
  schema_root_folder: string;
  ddl_file_encoding: string;
  ddl_file_extensions: Record<string, string>;
  storage_modes: Record<string, string>;
  naming_convention: string;
  code_folder_name: string;
  migration_folder_name: string;
  migration_folder_mode: string;
  migration_version_label: string;
  plsql_dev_path: string;
};

export type SaveSettingsResponse = {
  oracle_client_initialized: boolean;
};

export type SchemaFolderOverride = {
  id: number;
  schema_name: string;
  folder_path: string;
  encoding: string;
  extensions: string;
  storage_modes: string;
  naming_convention: string;
  code_folder_name: string;
  migration_folder_name: string;
  migration_folder_mode: string;
  migration_version_label: string;
};

export type HistoryNamingRule = {
  id: number;
  match_type: MatchType;
  pattern: string;
  enabled: boolean;
};

export type HistoryTableIssue = {
  history_table: string;
  column_name: string;
  issue_type: 'MISSING' | 'TYPE_MISMATCH';
  main_type: string;
  history_type: string;
};

export type HistoryFixResult = {
  issues: HistoryTableIssue[];
  fix_sql: string;
};

export type ServerHistoryFixResult = {
  server_id: number;
  server_name: string;
  issues: HistoryTableIssue[];
  fix_sql: string;
  error?: string;
};

export type FilterAction = 'exclude' | 'include';
export type MatchType = 'prefix' | 'suffix' | 'contains' | 'exact';

export type TableFilterRule = {
  id: number;
  action: FilterAction;
  match_type: MatchType;
  pattern: string;
  enabled: boolean;
};

export type SaveDdlResult = {
  code_path: string | null;
  migration_path: string | null;
};

export type McpDataAccess = 'none' | 'allowlist';

export type McpTableRule = {
  action: 'allow' | 'deny';
  table: string;
  /** Empty means the whole table (all columns). */
  column: string;
};

export type McpObjectDenyRule = {
  /** `*` or an object type such as TABLE or PACKAGE. */
  object_type: string;
  name: string;
};

export type McpPolicy = {
  connection_id: number;
  mcp_enabled: boolean;
  allow_schema_list: boolean;
  allow_ddl_read: boolean;
  allow_invalid_objects: boolean;
  allow_save_scripts: boolean;
  allow_compare_fix: boolean;
  data_access: McpDataAccess;
  allow_raw_sql: boolean;
  table_rules: McpTableRule[];
  object_deny: McpObjectDenyRule[];
  max_rows: number;
  query_timeout_secs: number;
  mcp_username: string;
  /** Output only: whether a password is stored for the dedicated MCP account. */
  mcp_password_set: boolean;
};

export type McpToolStatus = {
  tool: string;
  allowed: boolean;
  reason: string;
};

export type McpPreviewObject = {
  name: string;
  object_type: string;
  data_access: 'allowed' | 'restricted' | 'denied' | 'n/a';
};

export type McpPreview = {
  tools: McpToolStatus[];
  objects: McpPreviewObject[];
  hidden_objects: number;
  data_tools_disabled: boolean;
  objects_error: string | null;
};

export type McpAuditSinkKind = '' | 'syslog' | 'eventlog' | 'http';

export type McpSettings = {
  data_tools_disabled: boolean;
  audit_sink_kind: McpAuditSinkKind;
  audit_sink_target: string;
};

export type McpAuditOutcome = 'allowed' | 'denied' | 'error';

export type McpAuditEntry = {
  id: number;
  /** UTC, RFC 3339. */
  timestamp: string;
  tool: string;
  connection_id: number | null;
  connection_name: string;
  /** JSON text. */
  arguments: string;
  outcome: McpAuditOutcome;
  reason: string;
  row_count: number | null;
  bytes: number;
  duration_ms: number;
  client_name: string;
  prev_hash: string;
  hash: string;
};

export type McpAuditFilter = {
  connection_id?: number | null;
  tool?: string | null;
  outcome?: McpAuditOutcome | null;
  /** Case-insensitive text matched against the entry's target names (tables, objects). */
  target?: string | null;
  /** `YYYY-MM-DD` or RFC 3339. */
  from?: string | null;
  /** `YYYY-MM-DD` covers the whole day. */
  to?: string | null;
  limit?: number | null;
  offset?: number | null;
};

export type McpAuditPage = {
  /** Newest first. */
  entries: McpAuditEntry[];
  total: number;
};

export type McpAuditVerification = {
  ok: boolean;
  entries: number;
  problem: string | null;
  first_bad_id: number | null;
};
