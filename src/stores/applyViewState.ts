import { writable } from 'svelte/store';
import type { ScriptServerResult } from '../types';

export const applySelectedServers = writable<Set<number>>(new Set());
export const applyScriptText = writable('');
export const applyResults = writable<ScriptServerResult[]>([]);
// When the shown results were produced, and on how many servers.
export const applyLastRun = writable<{ at: string; servers: number } | null>(null);
