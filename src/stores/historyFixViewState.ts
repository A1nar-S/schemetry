import { writable } from 'svelte/store';
import type { ServerHistoryFixResult } from '../types';

export const selectedServers = writable<Set<string>>(new Set());
export const results         = writable<ServerHistoryFixResult[]>([]);
export const activeServer    = writable<string>('');
