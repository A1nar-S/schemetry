import { writable, derived, get, type Readable } from 'svelte/store';

export type ThemePreference = 'system' | 'dark' | 'light';
type Theme = 'dark' | 'light';

const STORAGE_KEY = 'schemetry-theme';
const query = window.matchMedia('(prefers-color-scheme: light)');

function loadPreference(): ThemePreference {
  const saved = localStorage.getItem(STORAGE_KEY);
  // Older versions stored only 'dark' or 'light'; anything else follows the OS.
  return saved === 'dark' || saved === 'light' ? saved : 'system';
}

function createTheme() {
  const { subscribe, set } = writable<ThemePreference>(loadPreference());
  return {
    subscribe,
    set(pref: ThemePreference) {
      if (pref === 'system') localStorage.removeItem(STORAGE_KEY);
      else localStorage.setItem(STORAGE_KEY, pref);
      set(pref);
    },
  };
}

/** The user's choice: 'system' (follow the OS), 'dark' or 'light'. */
export const theme = createTheme();

const systemLight = writable(query.matches);
query.addEventListener('change', e => systemLight.set(e.matches));

/** The theme actually in effect ('dark' or 'light'), with 'system' resolved. */
export const resolvedTheme: Readable<Theme> = derived([theme, systemLight], ([$theme, $light]) =>
  $theme === 'system' ? ($light ? 'light' : 'dark') : $theme,
);

// Keep the attribute in sync, including before first render.
resolvedTheme.subscribe(t => document.documentElement.setAttribute('data-theme', t));

export function currentTheme(): Theme {
  return get(resolvedTheme);
}
