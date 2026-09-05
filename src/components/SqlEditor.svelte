<script lang="ts">
  import { onMount } from 'svelte';
  import { EditorView, basicSetup } from 'codemirror';
  import { EditorState, Compartment } from '@codemirror/state';
  import { sql } from '@codemirror/lang-sql';
  import { githubLight, githubDark } from '@uiw/codemirror-theme-github';
  import { get } from 'svelte/store';
  import { resolvedTheme } from '../hooks/useTheme';

  /** Bound to the parent via `bind:value`. */
  export let value = '';
  /** Prevents user editing; still shows syntax highlighting. */
  export let readonly = false;
  /** CSS height string, e.g. "140px". Defaults to "140px". */
  export let height = '140px';

  let container: HTMLDivElement;
  let view: EditorView;
  let internalChange = false;

  const themeCompartment = new Compartment();

  function themeExt(t: 'dark' | 'light') {
    return t === 'dark' ? githubDark : githubLight;
  }

  onMount(() => {
    const initialTheme = get(resolvedTheme);

    view = new EditorView({
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          sql(),
          EditorState.readOnly.of(readonly),
          EditorView.editable.of(!readonly),
          themeCompartment.of(themeExt(initialTheme)),
          EditorView.updateListener.of((upd) => {
            if (upd.docChanged && !internalChange) {
              value = upd.state.doc.toString();
            }
          }),
        ],
      }),
      parent: container,
    });

    // React to theme-store changes after mount
    const unsub = resolvedTheme.subscribe((t) => {
      if (view) {
        view.dispatch({ effects: themeCompartment.reconfigure(themeExt(t)) });
      }
    });

    return () => {
      unsub();
      view?.destroy();
    };
  });

  // Sync external value changes (e.g. recall from history) → editor
  $: if (view) {
    const current = view.state.doc.toString();
    if (current !== value) {
      internalChange = true;
      view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
      internalChange = false;
    }
  }
</script>

<div class="sql-cm" class:sql-cm-readonly={readonly} style:height bind:this={container}></div>

<style>
  .sql-cm {
    width: 100%;
    min-height: 80px;
    overflow: auto;
    border-radius: 6px;
    resize: vertical;
  }

  .sql-cm-readonly {
    resize: none;
  }

  /* Make the CodeMirror editor fill the container div exactly */
  .sql-cm :global(.cm-editor) {
    height: 100%;
    border-radius: 6px;
    border: 1px solid var(--border-input);
    font-family: 'JetBrains Mono', 'Consolas', monospace;
    font-size: 13px;
  }

  .sql-cm :global(.cm-editor.cm-focused) {
    outline: none;
    border-color: var(--text-link);
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--text-link) 20%, transparent);
  }

  .sql-cm :global(.cm-scroller) {
    font-family: 'JetBrains Mono', 'Consolas', monospace;
    font-size: 13px;
    line-height: 1.65;
    overflow: auto;
  }

  /* Readonly: subtle visual cue, no cursor blink */
  .sql-cm :global(.cm-editor[aria-readonly="true"]) {
    border-color: var(--border);
    opacity: 0.9;
  }
</style>
