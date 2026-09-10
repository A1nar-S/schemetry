import { writable } from 'svelte/store';
import type { Discrepancy, FixScriptResult, LoadedServer } from '../types';

export const selectedForFetch  = writable<Set<number>>(new Set());
export const loadedServers     = writable<LoadedServer[]>([]);
export const referenceServer   = writable<number | null>(null);
export const checkComments     = writable(false);
export const checkIndexes      = writable(false);
export const discrepancies     = writable<Discrepancy[]>([]);
export const filterQuery       = writable('');
export const selectedIds       = writable<Set<number>>(new Set());
export const targetServer      = writable<number | null>(null);
export const generatedScripts  = writable<Map<number, FixScriptResult>>(new Map());
export const activeSqlServer   = writable<number | null>(null);
export const outputFolder      = writable('');
