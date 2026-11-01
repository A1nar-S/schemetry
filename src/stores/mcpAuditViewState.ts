import { writable } from 'svelte/store';
import type { McpAuditEntry, McpAuditVerification } from '../types';

export const auditConnection   = writable<number | null>(null);
export const auditTool         = writable('');
export const auditOutcome      = writable('');
export const auditTarget       = writable('');
export const auditFrom         = writable('');
export const auditTo           = writable('');
export const auditEntries      = writable<McpAuditEntry[]>([]);
export const auditTotal        = writable(0);
export const auditSelectedId   = writable<number | null>(null);
// Shared with App.svelte, which shows the integrity banner from it.
export const auditVerification = writable<McpAuditVerification | null>(null);
