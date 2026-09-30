<script lang="ts">
  import { createVirtualizer } from '@tanstack/svelte-virtual';

  /** Virtualized hex dump — renders only the visible rows, so the full value is shown. */
  export let bytes: Uint8Array;

  const ROW_H = 20;
  const PER_ROW = 16;

  let scrollEl: HTMLDivElement;

  $: rowCount = Math.ceil(bytes.length / PER_ROW);
  $: virt = createVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: rowCount,
    getScrollElement: () => scrollEl,
    estimateSize: () => ROW_H,
    overscan: 20,
  });
  $: offsetDigits = Math.max(8, bytes.length.toString(16).length);

  function hex(row: number): string {
    const start = row * PER_ROW;
    let s = '';
    for (let i = 0; i < PER_ROW; i++) {
      const at = start + i;
      s += at < bytes.length ? bytes[at].toString(16).padStart(2, '0').toUpperCase() : '  ';
      s += i === 7 ? '  ' : ' ';
    }
    return s;
  }

  function text(row: number): string {
    const chunk = bytes.subarray(row * PER_ROW, row * PER_ROW + PER_ROW);
    return [...chunk].map(b => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '.')).join('');
  }
</script>

<div class="hex-view" bind:this={scrollEl}>
  <div style="height:{$virt.getTotalSize()}px; position:relative;">
    {#each $virt.getVirtualItems() as item (item.key)}
      <div class="hex-row" style="height:{ROW_H}px; transform:translateY({item.start}px);">
        <span class="hex-offset">{(item.index * PER_ROW).toString(16).padStart(offsetDigits, '0').toUpperCase()}</span>
        <span class="hex-bytes">{hex(item.index)}</span>
        <span class="hex-text">{text(item.index)}</span>
      </div>
    {/each}
  </div>
</div>

<style>
  .hex-view {
    height: 100%;
    overflow: auto;
    border: 1px solid var(--border-subtle);
    border-radius: 6px;
    background: var(--bg-panel);
    font-family: 'Cascadia Mono', Consolas, 'Courier New', monospace;
    font-size: 12px;
  }
  .hex-row {
    position: absolute;
    top: 0;
    left: 0;
    display: flex;
    gap: 16px;
    padding: 0 10px;
    line-height: 20px;
    white-space: pre;
  }
  .hex-offset { color: var(--text-muted); }
  .hex-bytes { color: var(--text-primary); }
  .hex-text { color: var(--text-label); }
</style>
