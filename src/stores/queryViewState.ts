import { writable } from 'svelte/store';
import type { QueryServerResult, QueryHistoryEntry } from '../types';

export const selectedServers = writable<Set<string>>(new Set());
export const sql = writable('SELECT * FROM DUAL WHERE ROWNUM <= 10');
export const results = writable<QueryServerResult[]>([]);
// The SQL that produced the current `results` — used to lazily re-fetch BLOB cells.
export const lastRunSql = writable('');
export const activeServer = writable('');
export const history = writable<QueryHistoryEntry[]>([]);
export const historyOpen = writable(false);
export const lastExportDir = writable('');
// Excel export layout: 'per-server' = one worksheet tab per server,
// 'single' = all servers combined on one tab with a Server column.
export const exportMode = writable<'per-server' | 'single'>('per-server');
// When true, LOB columns are materialized inline (CLOB → text, BLOB → text/hex)
// instead of showing <CLOB>/<BLOB> placeholders, for both the grid and export.
export const showLobContent = writable(false);
