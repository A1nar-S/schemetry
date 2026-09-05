<script lang="ts" context="module">
  export type VCol = {
    key: string;
    header: string;
    width?: number;
    minWidth?: number;
    flex?: number; // treated as wide default
    html?: boolean;     // render cell content as HTML instead of plain text
    titleKey?: string;  // row key to use for the tooltip instead of key
  };
</script>

<script lang="ts">
  import { onDestroy } from 'svelte';
  import { createVirtualizer } from '@tanstack/svelte-virtual';
  import { indeterminate } from '../actions';

  export let columns: VCol[];
  export let rows: Record<string, unknown>[];
  export let selectable = false;
  export let selectedIds: Set<number> = new Set();
  export let getRowId: ((row: Record<string, unknown>) => number) | undefined = undefined;
  export let onSelectionChange: ((ids: Set<number>) => void) | undefined = undefined;
  export let getCellClass: ((row: Record<string, unknown>, colKey: string) => string) | undefined = undefined;
  // Called when a data cell is double-clicked (e.g. to open a full-value viewer).
  export let onCellActivate: ((row: Record<string, unknown>, colKey: string) => void) | undefined = undefined;

  const ROW_H = 32;
  const MIN_COL_W = 40;
  const CHECK_W = 36;

  // ─── Column widths (all pixel-based so resize works) ──────────────
  // flex columns start wide; plain width columns use their specified size.
  // Reset when the column set changes (e.g. QueryView gets results), but NOT
  // during a resize drag (which mutates colWidths without changing columns).
  let colWidths: number[] = [];
  let _prevColKeys = '';
  $: {
    const keys = columns.map((c) => c.key).join('\0');
    if (keys !== _prevColKeys) {
      _prevColKeys = keys;
      colWidths = columns.map((c) =>
        c.width ? c.width : Math.max(c.minWidth ?? MIN_COL_W, c.flex ? 200 : 160),
      );
    }
  }

  $: totalContentWidth =
    (selectable ? CHECK_W : 0) + colWidths.reduce((sum, w) => sum + w, 0);

  $: gridTemplate = [
    ...(selectable ? [`${CHECK_W}px`] : []),
    ...columns.map((col, i) => col.flex ? `minmax(${colWidths[i]}px, 1fr)` : `${colWidths[i]}px`),
  ].join(' ');

  // ─── Sort ─────────────────────────────────────────────────────────
  let sortKey = '';
  let sortDir: 'asc' | 'desc' = 'asc';

  $: sortedRows = sortKey
    ? [...rows].sort((a, b) => {
        const av = String(a[sortKey] ?? '');
        const bv = String(b[sortKey] ?? '');
        const cmp = av.localeCompare(bv, undefined, { numeric: true, sensitivity: 'base' });
        return sortDir === 'asc' ? cmp : -cmp;
      })
    : rows;

  function toggleSort(key: string) {
    if (sortKey === key) sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    else { sortKey = key; sortDir = 'asc'; }
  }

  // ─── Selection ────────────────────────────────────────────────────
  function toggleRow(rowId: number) {
    const next = new Set(selectedIds);
    if (next.has(rowId)) next.delete(rowId);
    else next.add(rowId);
    onSelectionChange?.(next);
  }

  function toggleAll() {
    if (selectedIds.size === sortedRows.length) {
      onSelectionChange?.(new Set());
    } else {
      onSelectionChange?.(new Set(sortedRows.map((row, i) => (getRowId ? getRowId(row) : i))));
    }
  }

  export function clearSelection() { onSelectionChange?.(new Set()); }

  // ─── Virtualizer ─────────────────────────────────────────────────
  // Track rowCount (not row reference) so sorting (same length) never
  // recreates the virtualizer — that was the source of the "freezy" feel.
  let scrollEl: HTMLDivElement;
  $: rowCount = sortedRows.length;
  $: virt = createVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: rowCount,
    getScrollElement: () => scrollEl,
    estimateSize: () => ROW_H,
    overscan: 12,
  });

  export function scrollToTop() {
    if (scrollEl) scrollEl.scrollTop = 0;
  }

  // Scroll to top only when the source dataset prop changes, not on sort.
  $: { rows; if (scrollEl) scrollEl.scrollTop = 0; }

  // ─── Column resize drag ───────────────────────────────────────────
  let resizingCol = -1;
  let resizeStartX = 0;
  let resizeStartW = 0;

  function onResizeStart(e: MouseEvent, colIdx: number) {
    e.preventDefault();
    e.stopPropagation();
    resizingCol = colIdx;
    resizeStartX = e.clientX;
    resizeStartW = colWidths[colIdx];
    window.addEventListener('mousemove', onResizeMove);
    window.addEventListener('mouseup', onResizeEnd, { once: true });
  }

  function onResizeMove(e: MouseEvent) {
    if (resizingCol < 0) return;
    const minW = columns[resizingCol]?.minWidth ?? MIN_COL_W;
    colWidths[resizingCol] = Math.max(minW, resizeStartW + (e.clientX - resizeStartX));
    colWidths = [...colWidths];
  }

  function onResizeEnd() {
    resizingCol = -1;
    window.removeEventListener('mousemove', onResizeMove);
  }

  onDestroy(() => {
    window.removeEventListener('mousemove', onResizeMove);
    window.removeEventListener('mouseup', onResizeEnd);
  });
</script>

<div class="vt-outer">
  <!--
    Single scroll container — header is position:sticky inside, so it:
    • sticks at the top on vertical scroll
    • moves with the content on horizontal scroll
  -->
  <div class="vt-scroll" bind:this={scrollEl}>
    <div class="vt-inner" style="min-width:{totalContentWidth}px;">
      <!-- Sticky header -->
      <div class="vt-header" style="grid-template-columns:{gridTemplate};">
        {#if selectable}
          <div class="vt-hcell vt-check-cell">
            <input
              type="checkbox"
              checked={selectedIds.size === sortedRows.length && sortedRows.length > 0}
              use:indeterminate={selectedIds.size > 0 && selectedIds.size < sortedRows.length}
              on:change={toggleAll}
            />
          </div>
        {/if}
        {#each columns as col, i}
          <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
          <div class="vt-hcell" on:click={() => toggleSort(col.key)}>
            <span class="vt-hcell-text">{col.header}</span>
            {#if sortKey === col.key}
              <span class="vt-sort-icon">{sortDir === 'asc' ? '↑' : '↓'}</span>
            {:else}
              <span class="vt-sort-hint">⇅</span>
            {/if}
            <!-- Resize handle — stops propagation so sort doesn't fire during drag -->
            <!-- svelte-ignore a11y-no-static-element-interactions -->
            <div
              class="vt-resize-handle"
              class:vt-resize-active={resizingCol === i}
              on:mousedown={(e) => onResizeStart(e, i)}
              on:click|stopPropagation
            ></div>
          </div>
        {/each}
      </div>

      <!-- Virtual body sizer -->
      <div style="height:{$virt.getTotalSize()}px; position:relative;">
        {#each $virt.getVirtualItems() as item (item.key)}
          {@const row = sortedRows[item.index]}
          {@const rowId = getRowId ? getRowId(row) : item.index}
          <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
          <div
            class="vt-row"
            class:vt-row-selected={selectedIds.has(rowId)}
            style="position:absolute;top:0;height:{ROW_H}px;transform:translateY({item.start}px);width:100%;display:grid;grid-template-columns:{gridTemplate};"
            on:click={selectable ? () => toggleRow(rowId) : undefined}
          >
            {#if selectable}
              <div class="vt-cell vt-check-cell">
                <input
                  type="checkbox"
                  checked={selectedIds.has(rowId)}
                  on:change={() => toggleRow(rowId)}
                  on:click|stopPropagation
                />
              </div>
            {/if}
            {#each columns as col}
              <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
              <div
                class="vt-cell {getCellClass ? getCellClass(row, col.key) : ''}"
                title={String(row[col.titleKey ?? col.key] ?? '')}
                on:dblclick={onCellActivate ? () => onCellActivate(row, col.key) : undefined}
              >
                {#if col.html}
                  {@html String(row[col.key] ?? '')}
                {:else}
                  {row[col.key] ?? ''}
                {/if}
              </div>
            {/each}
          </div>
        {/each}
      </div>
    </div>
  </div>
</div>
