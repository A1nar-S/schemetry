import { writable } from 'svelte/store';
import type { CompletionMetadata, QueryServerResult, QueryHistoryEntry } from '../types';

export const selectedServers = writable<Set<number>>(new Set());
export const sql = writable('SELECT * FROM DUAL WHERE ROWNUM <= 10');
export const results = writable<QueryServerResult[]>([]);
// The SQL that produced the current `results` — used to lazily re-fetch BLOB cells.
export const lastRunSql = writable('');
export const activeServer = writable<number | null>(null);
export const history = writable<QueryHistoryEntry[]>([]);
export const historyOpen = writable(false);
// Autocomplete metadata per server id, fetched once per session (or on refresh).
export const completionCache = writable<Record<number, CompletionMetadata>>({});
// Server picked for autocomplete (and the editor dialect); null = follow the active tab.
export const completionServerPick = writable<number | null>(null);
// Connection groups expanded in the sidebar; everything else starts collapsed.
// Persisted so the layout survives restarts.
const EXPANDED_GROUPS_KEY = 'schemetry-query-expanded-groups';
function loadExpandedGroups(): Set<string> {
  try {
    const saved = JSON.parse(localStorage.getItem(EXPANDED_GROUPS_KEY) ?? '[]');
    return new Set(Array.isArray(saved) ? saved.filter((g): g is string => typeof g === 'string') : []);
  } catch {
    return new Set();
  }
}
export const expandedGroups = writable<Set<string>>(loadExpandedGroups());
expandedGroups.subscribe(groups => {
  try {
    localStorage.setItem(EXPANDED_GROUPS_KEY, JSON.stringify([...groups]));
  } catch {
    // Storage unavailable — the state just won't persist.
  }
});
export const lastExportDir = writable('');
// Excel export layout: 'per-server' = one worksheet tab per server,
// 'single' = all servers combined on one tab with a Server column.
export const exportMode = writable<'per-server' | 'single'>('per-server');
// Last export file format picked in the save dialog; it's offered first next time.
const EXPORT_FORMAT_KEY = 'schemetry-query-export-format';
function loadExportFormat(): 'xlsx' | 'csv' {
  try {
    return localStorage.getItem(EXPORT_FORMAT_KEY) === 'csv' ? 'csv' : 'xlsx';
  } catch {
    return 'xlsx';
  }
}
export const exportFormat = writable<'xlsx' | 'csv'>(loadExportFormat());
exportFormat.subscribe(format => {
  try {
    localStorage.setItem(EXPORT_FORMAT_KEY, format);
  } catch {
    // Storage unavailable — the choice just won't persist.
  }
});
// When true, LOB columns are materialized inline (CLOB → text, BLOB → text/hex)
// instead of showing <CLOB>/<BLOB> placeholders, for both the grid and export.
export const showLobContent = writable(false);
