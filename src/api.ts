import { invoke } from '@tauri-apps/api/core';
import type {
  AppSettings,
  ConnectionRecord,
  Discrepancy,
  FetchServersResponse,
  FixScriptResult,
  HistoryNamingRule,
  LobContent,
  QueryHistoryEntry,
  QueryServerResult,
  SaveDdlResult,
  SaveSettingsResponse,
  SchemaFolderOverride,
  SchemaObject,
  ServerHistoryFixResult,
  TableFilterRule,
} from './types';

export async function getConnections(): Promise<ConnectionRecord[]> {
  return invoke('get_connections');
}

export async function saveConnection(payload: {
  editing_id?: number;
  connection: ConnectionRecord;
}): Promise<void> {
  return invoke('save_connection', { payload });
}

export async function deleteConnection(id: number): Promise<void> {
  return invoke('delete_connection', { id });
}

export async function deleteAllConnections(): Promise<void> {
  return invoke('delete_all_connections');
}

export async function testConnection(connection: ConnectionRecord): Promise<void> {
  return invoke('test_connection', { connection });
}

export async function importConnections(json: string): Promise<number> {
  return invoke('import_connections', { json });
}

export async function exportConnections(path: string): Promise<number> {
  return invoke('export_connections', { path });
}

export async function fetchServers(server_names: string[]): Promise<FetchServersResponse> {
  return invoke('fetch_servers', { serverNames: server_names });
}

export async function compareDiscrepancies(args: {
  reference_server: string;
  check_comments: boolean;
  check_indexes: boolean;
}): Promise<Discrepancy[]> {
  return invoke('compare_discrepancies', {
    referenceServer: args.reference_server,
    checkComments: args.check_comments,
    checkIndexes: args.check_indexes,
  });
}

export async function generateFixScript(args: {
  discrepancies: Discrepancy[];
  selected_ids: number[];
  reference_server: string;
}): Promise<FixScriptResult> {
  return invoke('generate_fix_script', {
    discrepancies: args.discrepancies,
    selectedIds: args.selected_ids,
    referenceServer: args.reference_server,
  });
}

export async function exportCompareReport(
  discrepancies: Discrepancy[],
  output_folder: string,
): Promise<[string, string]> {
  return invoke('export_compare_report', { discrepancies, outputFolder: output_folder });
}

export async function runQuery(
  sql: string,
  server_names: string[],
  materialize_lobs = false,
): Promise<QueryServerResult[]> {
  return invoke('run_query', { sql, serverNames: server_names, materializeLobs: materialize_lobs });
}

export async function getQueryHistory(): Promise<QueryHistoryEntry[]> {
  return invoke('get_query_history');
}

export async function fetchLobContent(
  server_name: string,
  sql: string,
  row_index: number,
  col_index: number,
): Promise<LobContent> {
  return invoke('fetch_lob_content', {
    serverName: server_name,
    sql,
    rowIndex: row_index,
    colIndex: col_index,
  });
}

export async function saveBlobToFile(
  server_name: string,
  sql: string,
  row_index: number,
  col_index: number,
  path: string,
): Promise<number> {
  return invoke('save_blob_to_file', {
    serverName: server_name,
    sql,
    rowIndex: row_index,
    colIndex: col_index,
    path,
  });
}

export async function deleteQueryHistoryItem(id: number): Promise<void> {
  return invoke('delete_query_history_item', { id });
}

export async function pinQueryHistoryItem(id: number, pinned: boolean): Promise<void> {
  return invoke('pin_query_history_item', { id, pinned });
}

export async function setQueryFavorite(id: number, favorite: boolean, description: string): Promise<void> {
  return invoke('set_query_favorite', { id, favorite, description });
}

export async function reorderFavorites(orderedIds: number[]): Promise<void> {
  return invoke('reorder_favorites', { orderedIds });
}

export async function clearQueryHistory(): Promise<void> {
  return invoke('clear_query_history');
}

export async function exportQueryResults(
  results: QueryServerResult[],
  output_path: string,
  single_sheet: boolean,
): Promise<void> {
  return invoke('export_query_results', { results, outputPath: output_path, singleSheet: single_sheet });
}

export async function fetchSchemaObjects(serverName: string): Promise<SchemaObject[]> {
  return invoke('fetch_schema_objects', { serverName });
}

export async function fetchObjectDdl(serverName: string, objectName: string, objectType: string): Promise<string> {
  return invoke('fetch_object_ddl', { serverName, objectName, objectType });
}

export async function generateHistoryFix(serverNames: string[]): Promise<ServerHistoryFixResult[]> {
  return invoke('generate_history_fix', { serverNames });
}

export async function getSettings(): Promise<AppSettings> {
  return invoke('get_settings');
}

export async function saveSettings(
  output_folder: string,
  client_lib_dir: string,
  schema_root_folder: string,
  ddl_file_encoding: string,
  ddl_file_extensions: Record<string, string>,
  storage_modes: Record<string, string>,
  naming_convention: string,
  code_folder_name: string,
  migration_folder_name: string,
  migration_folder_mode: string,
  migration_version_label: string,
  plsql_dev_path: string,
): Promise<SaveSettingsResponse> {
  return invoke('save_settings', {
    outputFolder: output_folder,
    clientLibDir: client_lib_dir,
    schemaRootFolder: schema_root_folder,
    ddlFileEncoding: ddl_file_encoding,
    ddlFileExtensions: ddl_file_extensions,
    storageModes: storage_modes,
    namingConvention: naming_convention,
    codeFolderName: code_folder_name,
    migrationFolderName: migration_folder_name,
    migrationFolderMode: migration_folder_mode,
    migrationVersionLabel: migration_version_label,
    plsqlDevPath: plsql_dev_path,
  });
}

export async function listHistoryNamingRules(): Promise<HistoryNamingRule[]> {
  return invoke('list_history_naming_rules');
}

export async function saveHistoryNamingRule(rule: HistoryNamingRule): Promise<HistoryNamingRule> {
  return invoke('save_history_naming_rule', { rule });
}

export async function deleteHistoryNamingRule(id: number): Promise<void> {
  return invoke('delete_history_naming_rule', { id });
}

export async function listSchemaFolderOverrides(): Promise<SchemaFolderOverride[]> {
  return invoke('list_folder_schema_overrides');
}

export async function saveSchemaFolderOverride(data: SchemaFolderOverride): Promise<SchemaFolderOverride> {
  return invoke('save_folder_schema_override', { data });
}

export async function deleteSchemaFolderOverride(id: number): Promise<void> {
  return invoke('delete_folder_schema_override', { id });
}

export async function listTableFilterRules(): Promise<TableFilterRule[]> {
  return invoke('list_table_filter_rules');
}

export async function saveTableFilterRule(rule: TableFilterRule): Promise<TableFilterRule> {
  return invoke('save_table_filter_rule', { rule });
}

export async function deleteTableFilterRule(id: number): Promise<void> {
  return invoke('delete_table_filter_rule', { id });
}

export async function saveDdlToFolder(args: {
  schema: string;
  object_name: string;
  object_type: string;
  ddl: string;
  description: string;
}): Promise<SaveDdlResult> {
  return invoke('save_ddl_to_folder', {
    schema: args.schema,
    objectName: args.object_name,
    objectType: args.object_type,
    ddl: args.ddl,
    description: args.description,
  });
}

export async function setOutputFolder(folder: string): Promise<void> {
  return invoke('set_output_folder', { folder });
}

export async function setLastQueryExportDir(dir: string): Promise<void> {
  return invoke('set_last_query_export_dir', { dir });
}

export async function openFolder(path: string): Promise<void> {
  return invoke('open_folder', { path });
}

export async function openFile(path: string): Promise<void> {
  return invoke('open_file', { path });
}

export async function openInVscode(path: string): Promise<void> {
  return invoke('open_in_vscode', { path });
}

export async function openSchemaInVscode(schema: string): Promise<void> {
  return invoke('open_schema_in_vscode', { schema });
}

export async function openInPlsqlDeveloper(connection: ConnectionRecord): Promise<void> {
  return invoke('open_in_plsql_developer', { connection });
}

