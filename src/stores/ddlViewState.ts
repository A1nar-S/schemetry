import { writable } from 'svelte/store';
import type { SchemaObject } from '../types';

export type DdlStep = 'pick-server' | 'objects';

export const selectedServer  = writable<string>('');
export const step            = writable<DdlStep>('pick-server');
export const objects         = writable<SchemaObject[]>([]);
export const filterQuery     = writable<string>('');
export const selectedObject  = writable<SchemaObject | null>(null);
export const generatedDdl    = writable<string>('');
export const collapsedTypes  = writable<Set<string>>(new Set());
