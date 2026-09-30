<script lang="ts">
  import Icon from './Icon.svelte';
  import { onDestroy } from 'svelte';
  import Modal from './Modal.svelte';
  import CodeView from './CodeView.svelte';
  import HexView from './HexView.svelte';
  import { decodeUtf8, detectFormat, extForMime, type LobFormat, type LobTab } from '../lobFormats';

  export let title: string;
  /** Column type, e.g. "BLOB". */
  export let typeLabel = '';
  export let loading = false;
  /** Text content (CLOBs, plain cells) — set this or `bytes`. */
  export let text: string | null = null;
  /** Binary content (BLOBs). */
  export let bytes: Uint8Array | null = null;
  /** Bytes/characters loaded. */
  export let size = 0;
  /** Only a prefix of the value was loaded. */
  export let truncated = false;
  /** Saves the full value; receives the extension for the detected format. */
  export let onSave: ((ext: string) => void) | undefined = undefined;
  export let saving = false;
  export let onClose: () => void;

  const TAB_LABELS: Record<LobTab, string> = {
    text: 'Text', formatted: 'Formatted', html: 'HTML', image: 'Image', pdf: 'PDF', word: 'Word', hex: 'Hex',
  };

  // Rendered HTML can't run scripts (sandbox) or load anything remote (CSP).
  const PREVIEW_CSP =
    `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; ` +
    `img-src data: blob:; style-src 'unsafe-inline'; font-src data:">`;

  let format: LobFormat | null = null;
  let tab: LobTab = 'text';
  let wrap = false;

  $: format = loading
    ? null
    : text !== null
      ? detectFormat({ text })
      : bytes
        ? detectFormat({ bytes, complete: !truncated })
        : null;
  $: tab = format?.initial ?? 'text';

  $: shownText = text ?? (bytes ? decodeUtf8(bytes) : null) ?? '';
  $: hexBytes = bytes ?? new TextEncoder().encode(text ?? '');

  // Blob URL for the image/PDF previews.
  let blobUrl = '';
  $: setBlobUrl(format, hexBytes);
  function setBlobUrl(f: LobFormat | null, data: Uint8Array) {
    if (blobUrl) URL.revokeObjectURL(blobUrl);
    blobUrl = f && (f.tabs.includes('image') || f.tabs.includes('pdf'))
      ? URL.createObjectURL(new Blob([data as Uint8Array<ArrayBuffer>], { type: f.mime }))
      : '';
  }
  onDestroy(() => { if (blobUrl) URL.revokeObjectURL(blobUrl); });

  let wordError = '';
  async function renderWord(e: Event) {
    const doc = (e.currentTarget as HTMLIFrameElement).contentDocument;
    if (!doc || !bytes) return;
    wordError = '';
    try {
      // Loaded on demand — it's the viewer's largest dependency.
      const { renderAsync } = await import('docx-preview');
      await renderAsync(bytes, doc.body, doc.head, {
        inWrapper: true,
        useBase64URL: true,
        renderAltChunks: false, // embedded raw HTML
      });
    } catch (err) {
      wordError = `Couldn't render this document: ${String(err)}`;
    }
  }

  function copyText() {
    navigator.clipboard.writeText(tab === 'formatted' && format?.formatted ? format.formatted : shownText);
  }

  function fmtSize(n: number): string {
    if (text !== null) return `${n.toLocaleString()} chars`;
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<Modal width="1100px" {onClose}>
  <div class="modal-header">
    <span class="modal-title">
      {title}
      <span class="lob-meta">
        {[typeLabel, format?.label, format ? fmtSize(size) : ''].filter(Boolean).join(' · ')}
      </span>
    </span>
    <button class="btn-secondary" on:click={onClose} title="Close"><Icon name="x" /></button>
  </div>

  {#if loading}
    <div class="lob-body empty-state">Loading content…</div>
  {:else if !format}
    <div class="lob-body empty-state">No content.</div>
  {:else}
    <div class="lob-toolbar">
      <div class="tab-strip">
        {#each format.tabs as t}
          <button class="tab-btn" class:active={tab === t} on:click={() => (tab = t)}>{TAB_LABELS[t]}</button>
        {/each}
      </div>
      <div class="lob-actions">
        {#if tab === 'text' || tab === 'formatted'}
          <label class="lob-wrap"><input type="checkbox" bind:checked={wrap} /> Wrap</label>
          <button class="btn-secondary" on:click={copyText}><Icon name="copy" /> Copy</button>
        {/if}
        {#if onSave}
          <button class="btn-primary" disabled={saving} on:click={() => onSave?.(extForMime(format?.mime ?? ''))}>
            <Icon name="save" /> Save to file…
          </button>
        {/if}
      </div>
    </div>

    {#if truncated}
      <div class="lob-note">
        Showing the first {fmtSize(size)} only{onSave ? ' — use “Save to file…” for the full value' : ''}.
      </div>
    {/if}

    <div class="lob-body">
      {#if tab === 'text'}
        <CodeView value={shownText} language={format.language} {wrap} />
      {:else if tab === 'formatted'}
        <CodeView value={format.formatted ?? ''} language={format.language} {wrap} />
      {:else if tab === 'html'}
        <iframe title="HTML preview" class="lob-frame lob-frame-light" sandbox="" srcdoc={PREVIEW_CSP + shownText}></iframe>
      {:else if tab === 'image' && blobUrl}
        <div class="lob-image"><img src={blobUrl} alt={title} /></div>
      {:else if tab === 'pdf' && blobUrl}
        <iframe title="PDF preview" class="lob-frame" src={blobUrl}></iframe>
      {:else if tab === 'word'}
        {#if wordError}
          <div class="empty-state">{wordError}</div>
        {:else}
          <!-- Scripts stay off in the rendered document; same-origin only lets us fill it. -->
          <iframe
            title="Word preview"
            class="lob-frame lob-frame-light"
            sandbox="allow-same-origin"
            srcdoc={PREVIEW_CSP}
            on:load={renderWord}
          ></iframe>
        {/if}
      {:else if tab === 'hex'}
        <HexView bytes={hexBytes} />
      {/if}
    </div>
  {/if}
</Modal>

<style>
  .lob-meta {
    margin-left: 8px;
    font-size: 12px;
    font-weight: 400;
    color: var(--text-muted);
  }
  .lob-toolbar {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 8px;
  }
  .lob-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-bottom: 4px;
  }
  .lob-wrap {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-label);
    cursor: pointer;
  }
  .lob-note {
    margin-bottom: 8px;
    font-size: 12px;
    color: var(--text-warn);
  }
  .lob-body {
    height: 65vh;
  }
  .lob-frame {
    width: 100%;
    height: 100%;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
  }
  /* Documents are authored for a white page. */
  .lob-frame-light {
    background: #fff;
  }
  .lob-image {
    height: 100%;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: auto;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    /* Checkerboard so transparent images stay visible. */
    background: repeating-conic-gradient(#8882 0% 25%, transparent 0% 50%) 50% / 16px 16px;
  }
  .lob-image img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
  }
</style>
