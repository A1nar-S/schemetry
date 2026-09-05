export type SchemaObject = {
  name: string;
  object_type: string;
};

export type ConnectionRecord = {
  id: number;
  name: string;
  host: string;
  port: number;
  service_name: string;
  username: string;
  password: string;
  group_name: string;
  password_broken?: boolean;
};

export type QueryServerResult = {
  server_name: string;
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
};

export type Discrepancy = {
  difference: string;
  element: string;
  table_name: string;
  column_name: string;
  server_name: string;
  details: string;
};

export type FetchServersResponse = {
  loaded_servers: string[];
  errors: { server: string; error: string }[];
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
