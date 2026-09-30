<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import { EditorView, basicSetup } from 'codemirror';
  import { EditorState, Compartment, type Extension } from '@codemirror/state';
  import { json } from '@codemirror/lang-json';
  import { xml } from '@codemirror/lang-xml';
  import { html } from '@codemirror/lang-html';
  import { githubLight, githubDark } from '@uiw/codemirror-theme-github';
  import { get } from 'svelte/store';
  import { resolvedTheme } from '../hooks/useTheme';

  /** Read-only text view with syntax highlighting, folding and Ctrl+F search. */
  export let value = '';
  export let language: 'json' | 'xml' | 'html' | null = null;
  export let wrap = false;

  let container: HTMLDivElement;
  let view: EditorView | undefined;

  const theme = new Compartment();
  const lang = new Compartment();
  const wrapping = new Compartment();

  const themeExt = (t: 'dark' | 'light') => (t === 'dark' ? githubDark : githubLight);
  const langExt = (l: typeof language): Extension =>
    l === 'json' ? json() : l === 'xml' ? xml() : l === 'html' ? html() : [];
  const wrapExt = (w: boolean): Extension => (w ? EditorView.lineWrapping : []);

  onMount(() => {
    view = new EditorView({
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          EditorState.readOnly.of(true),
          theme.of(themeExt(get(resolvedTheme))),
          lang.of(langExt(language)),
          wrapping.of(wrapExt(wrap)),
          EditorView.theme({ '&': { height: '100%' }, '.cm-scroller': { overflow: 'auto' } }),
        ],
      }),
      parent: container,
    });
    const unsub = resolvedTheme.subscribe(t => view?.dispatch({ effects: theme.reconfigure(themeExt(t)) }));
    return unsub;
  });

  onDestroy(() => view?.destroy());

  $: if (view && value !== view.state.doc.toString()) {
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value } });
  }
  $: view?.dispatch({ effects: lang.reconfigure(langExt(language)) });
  $: view?.dispatch({ effects: wrapping.reconfigure(wrapExt(wrap)) });
</script>

<div class="code-view" bind:this={container}></div>

<style>
  .code-view {
    height: 100%;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    overflow: hidden;
  }
</style>
