import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';
import svelteConfig from './svelte.config.js';

export default ts.config(
  {
    ignores: ['dist/', 'node_modules/', 'src-tauri/', 'docker/'],
  },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  {
    languageOptions: {
      globals: { ...globals.browser },
    },
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_', caughtErrorsIgnorePattern: '^_' },
      ],
      '@typescript-eslint/no-unused-expressions': ['error', { allowShortCircuit: true, allowTernary: true }],
    },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      parserOptions: {
        projectService: true,
        extraFileExtensions: ['.svelte'],
        parser: ts.parser,
        svelteConfig,
      },
    },
    rules: {
      // Components use legacy `$:` reactivity: bare reads like `$: { rows; filters; … }` declare
      // dependencies, and `let x = init; $: x = …` looks like a dead store to ESLint.
      '@typescript-eslint/no-unused-expressions': 'off',
      'no-useless-assignment': 'off',
      // Maps/Sets here are rebuilt and reassigned (or copied into stores), not mutated in place.
      'svelte/prefer-svelte-reactivity': 'off',
      // Adding keys to existing `{#each}` blocks risks duplicate-key runtime errors; key new ones.
      'svelte/require-each-key': 'off',
    },
  },
  {
    files: ['**/*.cjs'],
    languageOptions: { globals: { ...globals.node } },
  },
  {
    files: ['**/*.d.ts'],
    rules: { '@typescript-eslint/triple-slash-reference': 'off' },
  },
  {
    // Icon renders static SVG paths from ICONS; VirtualTable's `html` columns are
    // built by callers that must escape data (see FixView's escapeHtml).
    files: ['src/components/Icon.svelte', 'src/components/VirtualTable.svelte'],
    rules: { 'svelte/no-at-html-tags': 'off' },
  },
);
