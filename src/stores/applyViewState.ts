import { writable } from 'svelte/store';
import type { ScriptServerResult } from '../types';

export const applySelectedServers = writable<Set<number>>(new Set());
export const applyScriptText = writable('');
export const applyResults = writable<ScriptServerResult[]>([]);
// The script and servers behind `applyResults`, so the summary stays truthful after edits.
export const applyLastRun = writable<{ at: string; servers: number } | null>(null);
