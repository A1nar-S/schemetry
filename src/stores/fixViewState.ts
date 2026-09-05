import { writable } from 'svelte/store';
import type { Discrepancy, FixScriptResult } from '../types';

export const selectedForFetch  = writable<Set<string>>(new Set());
export const loadedServers     = writable<string[]>([]);
export const referenceServer   = writable('');
export const checkComments     = writable(false);
export const checkIndexes      = writable(false);
export const discrepancies     = writable<Discrepancy[]>([]);
export const filterQuery       = writable('');
export const selectedIds       = writable<Set<number>>(new Set());
export const targetServer      = writable('');
export const generatedScripts  = writable<Map<string, FixScriptResult>>(new Map());
export const activeSqlServer   = writable('');
export const outputFolder      = writable('');
