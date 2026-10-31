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
  import Icon from './Icon.svelte';
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
  // Case-insensitive substring match, applied in memory.
  export let filterable = false;
  // SQL-IDE style row selection: click selects a row, Ctrl/Shift+click extend it,
  // arrow keys move it, Ctrl+C copies the selected rows as tab-separated text.
  export let rowSelect = false;
  // Show a row-number column that stays pinned while scrolling sideways.
  export let rowNumbers = false;

  const ROW_H = 32;
  const MIN_COL_W = 40;
  const CHECK_W = 36;
  const ROWNUM_W = 52;

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
      filters = {};
    }
  }

  $: totalContentWidth =
    (selectable ? CHECK_W : 0) + (rowNumbers ? ROWNUM_W : 0) + colWidths.reduce((sum, w) => sum + w, 0);

  $: gridTemplate = [
    ...(selectable ? [`${CHECK_W}px`] : []),
    ...(rowNumbers ? [`${ROWNUM_W}px`] : []),
    ...columns.map((col, i) => col.flex ? `minmax(${colWidths[i]}px, 1fr)` : `${colWidths[i]}px`),
  ].join(' ');

  // ─── Filter ───────────────────────────────────────────────────────
  let filters: Record<string, string> = {};

  $: activeFilters = Object.entries(filters).filter(([, v]) => v.trim() !== '');
  $: filteredRows = activeFilters.length
    ? rows.filter((row) =>
        activeFilters.every(([key, v]) =>
          String(row[key] ?? '').toLowerCase().includes(v.trim().toLowerCase()),
        ),
      )
    : rows;

  // ─── Sort ─────────────────────────────────────────────────────────
  let sortKey = '';
  let sortDir: 'asc' | 'desc' = 'asc';

  $: sortedRows = sortKey
    ? [...filteredRows].sort((a, b) => {
        const av = String(a[sortKey] ?? '');
        const bv = String(b[sortKey] ?? '');
        const cmp = av.localeCompare(bv, undefined, { numeric: true, sensitivity: 'base' });
        return sortDir === 'asc' ? cmp : -cmp;
      })
    : filteredRows;

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

  // ─── Row selection (rowSelect) ────────────────────────────────────
  // Tracked by row object so it survives sorting and filtering.
  let picked = new Set<Record<string, unknown>>();
  let anchor: Record<string, unknown> | null = null; // start of a Shift range
  let cursor: Record<string, unknown> | null = null; // row moved by the arrow keys

  // A new result set clears the selection.
  $: { rows; picked = new Set(); anchor = null; cursor = null; }

  function pickRow(e: MouseEvent, row: Record<string, unknown>) {
    if (!rowSelect) return;
    scrollEl?.focus({ preventScroll: true });
    if (e.shiftKey && anchor) {
      selectRange(anchor, row, e.ctrlKey || e.metaKey);
    } else if (e.ctrlKey || e.metaKey) {
      const next = new Set(picked);
      if (next.has(row)) next.delete(row);
      else next.add(row);
      picked = next;
      anchor = row;
    } else {
      picked = new Set([row]);
      anchor = row;
    }
    cursor = row;
  }

  function selectRange(from: Record<string, unknown>, to: Record<string, unknown>, keep: boolean) {
    const a = sortedRows.indexOf(from);
    const b = sortedRows.indexOf(to);
    if (a < 0 || b < 0) { picked = new Set([to]); anchor = to; return; }
    const next = keep ? new Set(picked) : new Set<Record<string, unknown>>();
    for (let i = Math.min(a, b); i <= Math.max(a, b); i++) next.add(sortedRows[i]);
    picked = next;
  }

  // Shift+click would otherwise select the cell text between the two clicks.
  function onRowMouseDown(e: MouseEvent) {
    if (rowSelect && e.shiftKey) e.preventDefault();
  }

  function onGridKeydown(e: KeyboardEvent) {
    if (!rowSelect || !sortedRows.length) return;
    if (e.target instanceof HTMLInputElement) return; // typing in a filter box
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      picked = new Set(sortedRows);
      return;
    }
    if (mod && e.key.toLowerCase() === 'c') {
      if (!picked.size || window.getSelection()?.toString()) return; // let normal text copy win
      e.preventDefault();
      void navigator.clipboard?.writeText(selectedRowsAsText()).catch(() => {});
      return;
    }
    if (e.key === 'Escape') {
      picked = new Set(); anchor = null; cursor = null;
      return;
    }
    const step = { ArrowDown: 1, ArrowUp: -1, PageDown: 20, PageUp: -20 }[e.key];
    if (step === undefined && e.key !== 'Home' && e.key !== 'End') return;
    e.preventDefault();
    const cur = cursor ? sortedRows.indexOf(cursor) : -1;
    let idx = e.key === 'Home' ? 0 : e.key === 'End' ? sortedRows.length - 1 : cur < 0 ? 0 : cur + (step ?? 0);
    idx = Math.max(0, Math.min(sortedRows.length - 1, idx));
    const row = sortedRows[idx];
    if (e.shiftKey && anchor) selectRange(anchor, row, false);
    else { picked = new Set([row]); anchor = row; }
    cursor = row;
    $virt.scrollToIndex(idx, { align: 'auto' });
  }

  // Selected rows in on-screen order, one line per row, tab between values.
  function selectedRowsAsText(): string {
    const clean = (v: unknown) => String(v ?? '').replace(/[\t\r\n]+/g, ' ');
    return sortedRows
      .filter(r => picked.has(r))
      .map(r => columns.map(c => clean(r[c.key])).join('\t'))
      .join('\n');
  }

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

  // Scroll to top only when the source dataset prop or filters change, not on sort.
  $: { rows; filters; if (scrollEl) scrollEl.scrollTop = 0; }

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
  <!-- svelte-ignore a11y_no_noninteractive_tabindex a11y_no_static_element_interactions -->
  <div
    class="vt-scroll"
    class:vt-row-select={rowSelect}
    bind:this={scrollEl}
    tabindex={rowSelect ? 0 : undefined}
    on:keydown={onGridKeydown}
  >
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
        {#if rowNumbers}
          <div class="vt-hcell vt-rownum" title="Row number">#</div>
        {/if}
        {#each columns as col, i}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <div class="vt-hcell" on:click={() => toggleSort(col.key)}>
            <span class="vt-hcell-text">{col.header}</span>
            {#if sortKey === col.key}
              <span class="vt-sort-icon"><Icon name={sortDir === 'asc' ? 'arrow-up' : 'arrow-down'} size={12} /></span>
            {:else}
              <span class="vt-sort-hint"><Icon name="arrow-up-down" size={12} /></span>
            {/if}
            <!-- Resize handle — stops propagation so sort doesn't fire during drag -->
            <!-- svelte-ignore a11y_no_static_element_interactions -->
            <div
              class="vt-resize-handle"
              class:vt-resize-active={resizingCol === i}
              on:mousedown={(e) => onResizeStart(e, i)}
              on:click|stopPropagation
            ></div>
          </div>
        {/each}
      </div>

      <!-- Filter row -->
      {#if filterable}
        <div class="vt-filter-row" style="grid-template-columns:{gridTemplate};">
          {#if selectable}
            <div class="vt-fcell vt-check-cell"></div>
          {/if}
          {#if rowNumbers}
            <div class="vt-fcell vt-rownum"></div>
          {/if}
          {#each columns as col}
            <div class="vt-fcell">
              <input
                type="text"
                placeholder="Filter…"
                bind:value={filters[col.key]}
                on:click|stopPropagation
                on:mousedown|stopPropagation
              />
              {#if filters[col.key]}
                <button
                  type="button"
                  class="vt-fclear"
                  title="Clear filter"
                  on:click|stopPropagation={() => (filters[col.key] = '')}
                  on:mousedown|stopPropagation
                ><Icon name="x" size={11} /></button>
              {/if}
            </div>
          {/each}
        </div>
      {/if}

      <!-- Virtual body sizer -->
      <div style="height:{$virt.getTotalSize()}px; position:relative;">
        {#each $virt.getVirtualItems() as item (item.key)}
          {@const row = sortedRows[item.index]}
          {@const rowId = getRowId ? getRowId(row) : item.index}
          <!-- svelte-ignore a11y_click_events_have_key_events a11y_no_static_element_interactions -->
          <div
            class="vt-row"
            class:vt-row-selected={selectable ? selectedIds.has(rowId) : picked.has(row)}
            class:vt-row-cursor={rowSelect && cursor === row}
            style="position:absolute;top:0;height:{ROW_H}px;transform:translateY({item.start}px);width:100%;display:grid;grid-template-columns:{gridTemplate};"
            on:click={selectable ? () => toggleRow(rowId) : rowSelect ? (e) => pickRow(e, row) : undefined}
            on:mousedown={onRowMouseDown}
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
            {#if rowNumbers}
              <div class="vt-cell vt-rownum">{item.index + 1}</div>
            {/if}
            {#each columns as col}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
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
