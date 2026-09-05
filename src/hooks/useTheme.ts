import { writable, get, type Readable } from 'svelte/store';

type Theme = 'dark' | 'light';

const STORAGE_KEY = 'schemetry-theme';

function createTheme() {
  const initial: Theme =
    (localStorage.getItem(STORAGE_KEY) as Theme | null) ??
    (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');

  const { subscribe, set } = writable<Theme>(initial);

  // Apply before first render
  document.documentElement.setAttribute('data-theme', initial);

  function apply(t: Theme) {
    document.documentElement.setAttribute('data-theme', t);
    localStorage.setItem(STORAGE_KEY, t);
    set(t);
  }

  return {
    subscribe,
    toggle() {
      apply(get({ subscribe }) === 'dark' ? 'light' : 'dark');
    },
    set: apply,
  };
}

/** Reactive theme handle. Subscribe with `$theme` in Svelte, or `.subscribe()` in TS. */
export const theme = createTheme();

/** Read-only alias for consumers that only need the current value, not `toggle()`/`set()`. */
export const resolvedTheme: Readable<Theme> = theme;
