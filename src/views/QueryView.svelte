<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { get } from 'svelte/store';
  import type { ConnectionRecord } from '../types';
  import type { VCol } from '../components/VirtualTable.svelte';
  import VirtualTable from '../components/VirtualTable.svelte';
  import SqlEditor from '../components/SqlEditor.svelte';
  import { busy, setBusy, notify } from '../stores/notification';
  import { save } from '@tauri-apps/plugin-dialog';
  import {
    clearQueryHistory,
    deleteQueryHistoryItem,
    exportQueryResults,
    fetchLobContent,
    saveBlobToFile,
    getQueryHistory,
    getSettings,
    pinQueryHistoryItem,
    setQueryFavorite,
    reorderFavorites,
    runQuery,
    setLastQueryExportDir,
  } from '../api';
  import Modal from '../components/Modal.svelte';
  import type { QueryHistoryEntry, QueryServerResult } from '../types';
  import {
    selectedServers,
    sql,
    results,
    activeServer,
    history,
    historyOpen,
    lastExportDir,
    exportMode,
    lastRunSql,
    showLobContent,
  } from '../stores/queryViewState';

  export let connections: ConnectionRecord[];

  // ── Schema groups ──────────────────────────────────────────────────
  $: schemaGroups = buildSchemaGroups(connections);

  function buildSchemaGroups(conns: ConnectionRecord[]): Map<string, ConnectionRecord[]> {
    const groups = new Map<string, ConnectionRecord[]>();
    for (const c of conns) {
      const key = c.group_name?.trim() || 'Default';
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(c);
    }
    return groups;
  }

  function selectSchema(schema: string) {
    selectedServers.update(s => { (schemaGroups.get(schema) ?? []).forEach(c => s.add(c.name)); return new Set(s); });
  }
  function deselectSchema(schema: string) {
    selectedServers.update(s => { (schemaGroups.get(schema) ?? []).forEach(c => s.delete(c.name)); return new Set(s); });
  }
  function toggleServer(name: string) {
    selectedServers.update(s => { s.has(name) ? s.delete(name) : s.add(name); return new Set(s); });
  }
  function selectAll() {
    selectedServers.set(new Set(connections.map(c => c.name)));
  }
  function selectNone() {
    selectedServers.set(new Set());
  }

  // ── VirtualTable data ─────────────────────────────────────────────
  $: activeResult = $results.find(r => r.server_name === $activeServer);
  $: singleView = $exportMode === 'single';

  // The result whose columns/types define the grid: active server, or the first
  // server with data when showing all servers in one combined table.
  $: baseResult = singleView ? $results.find(r => r.columns.length) : activeResult;

  // Map of column name → Oracle type label, used to flag openable LOB cells.
  $: colType = baseResult
    ? Object.fromEntries(baseResult.columns.map((name, i) => [name, baseResult!.column_types?.[i] ?? '']))
    : {};

  // 'binary' = BLOB-family (rich viewer), 'text' = CLOB-family (text viewer), null = plain.
  function lobKind(colKey: string): 'binary' | 'text' | null {
    const t = colType[colKey];
    if (t === 'BLOB' || t === 'BFILE' || t === 'LONG RAW') return 'binary';
    if (t === 'CLOB' || t === 'NCLOB' || t === 'LONG') return 'text';
    return null;
  }

  function cellClass(_row: Record<string, unknown>, colKey: string): string {
    return lobKind(colKey) ? 'vt-cell-lob' : '';
  }

  $: vtCols = baseResult
    ? [
        ...(singleView ? [{ key: '__server', header: 'Server', width: 160 } as VCol] : []),
        ...baseResult.columns.map((name): VCol => ({ key: name, header: name, flex: 1, minWidth: 100 })),
      ]
    : [];

  // Each grid row carries its origin server (`__server`) and the row index within
  // that server's result (`__rowIndex`) so the LOB viewer can re-fetch the right cell.
  function mapRows(r: QueryServerResult): Record<string, string | number | null>[] {
    return r.rows.map((row, idx) => {
      const out: Record<string, string | number | null> = { __rowIndex: idx, __server: r.server_name };
      r.columns.forEach((col, i) => { out[col] = row[i] ?? null; });
      return out;
    });
  }

  $: gridRows = singleView
    ? $results.filter(r => r.columns.length).flatMap(mapRows)
    : activeResult
      ? mapRows(activeResult)
      : [];

  onMount(async () => {
    // Only load on first mount (store already has data on subsequent visits)
    if (!get(history).length) {
      history.set(await getQueryHistory().catch(() => []));
    }
    if (!get(lastExportDir)) {
      const s = await getSettings().catch(() => ({ output_folder: '', client_lib_dir: '', last_query_export_dir: '' }));
      lastExportDir.set(s.last_query_export_dir);
    }
  });

  // ── Handlers ───────────────────────────────────────────────────────
  async function onRunQuery() {
    await executeQuery(get(sql));
  }

  async function executeQuery(sqlText: string) {
    const servers = get(selectedServers);
    if (!sqlText.trim()) { notify('SQL cannot be empty.', 'error'); return; }
    if (!servers.size) { notify('Select at least one server.', 'error'); return; }
    setBusy(true, 'Running query…');
    try {
      const queryResults = await runQuery(sqlText, [...servers], get(showLobContent));
      results.set(queryResults);
      lastRunSql.set(sqlText);
      activeServer.set(queryResults[0]?.server_name ?? '');
      history.set(await getQueryHistory());
      notify(`Query executed on ${queryResults.length} server(s).`, 'ok');
    } catch (e) {
      notify(`Query failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  // Re-run the last query when the LOB-content mode changes, so the grid refreshes.
  function onToggleLobContent() {
    const last = get(lastRunSql);
    if (last && get(selectedServers).size && !get(busy)) {
      void executeQuery(last);
    }
  }

  async function onExport() {
    const lastDir = get(lastExportDir);
    const filePath = await save({
      title: 'Export Query Results',
      defaultPath: lastDir ? `${lastDir}\\query_export.xlsx` : 'query_export.xlsx',
      filters: [{ name: 'Excel', extensions: ['xlsx'] }],
    });
    if (!filePath) return;
    const dir = filePath.replace(/[/\\][^/\\]+$/, '');
    if (dir !== lastDir) {
      lastExportDir.set(dir);
      setLastQueryExportDir(dir).catch(() => {});
    }
    setBusy(true, 'Exporting results…');
    try {
      await exportQueryResults(get(results), filePath, get(exportMode) === 'single');
      notify(`Exported to ${filePath}`, 'ok', dir, filePath);
    } catch (e) {
      notify(`Export failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onDeleteHistory(id: number) {
    await deleteQueryHistoryItem(id).catch(() => {});
    history.set(await getQueryHistory().catch(() => []));
  }

  async function onPinHistory(id: number, pinned: boolean) {
    await pinQueryHistoryItem(id, pinned).catch(() => {});
    history.set(await getQueryHistory().catch(() => []));
  }

  async function onClearHistory() {
    await clearQueryHistory().catch(() => {});
    history.set(await getQueryHistory().catch(() => []));
    notify('Query history cleared.', 'ok');
  }

  function recallQuery(sqlText: string) {
    sql.set(sqlText);
  }

  function fmtDuration(ms: number): string {
    return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(1)} s`;
  }

  // ── Cell value viewer ──────────────────────────────────────────────
  let cellViewerOpen = false;
  let cellViewerTitle = '';
  let cellViewerKind: 'plain' | 'text' | 'binary' = 'plain';
  let cellViewerText = '';        // plain cell value or loaded CLOB text
  let cellViewerRowIndex = -1;
  let cellViewerColIndex = -1;
  let cellViewerServer = '';
  let cellViewerLoading = false;
  let cellViewerLoaded = false;   // LOB content fetched?
  let lobMime = '';
  let lobBase64 = '';
  let lobBlobUrl = '';
  let lobTruncated = false;
  let lobSize = 0;

  function resetLobState() {
    if (lobBlobUrl) { URL.revokeObjectURL(lobBlobUrl); lobBlobUrl = ''; }
    cellViewerLoading = false;
    cellViewerLoaded = false;
    lobMime = '';
    lobBase64 = '';
    lobTruncated = false;
    lobSize = 0;
  }

  function closeCellViewer() {
    cellViewerOpen = false;
    resetLobState();
  }

  function onCellActivate(row: Record<string, unknown>, colKey: string) {
    resetLobState();
    cellViewerTitle = colKey;
    cellViewerText = '';
    cellViewerRowIndex = typeof row.__rowIndex === 'number' ? row.__rowIndex : -1;
    cellViewerColIndex = baseResult ? baseResult.columns.indexOf(colKey) : -1;
    cellViewerServer = typeof row.__server === 'string' ? row.__server : get(activeServer);

    const kind = lobKind(colKey);
    if (kind === null) {
      const value = row[colKey];
      cellViewerKind = 'plain';
      cellViewerText = value == null ? '' : String(value);
      cellViewerOpen = true;
      return;
    }
    // LOB cell — open and fetch content on demand.
    cellViewerKind = kind;
    cellViewerOpen = true;
    void loadLobContent();
  }

  async function loadLobContent() {
    if (cellViewerRowIndex < 0 || cellViewerColIndex < 0) return;
    cellViewerLoading = true;
    try {
      const content = await fetchLobContent(
        cellViewerServer,
        get(lastRunSql),
        cellViewerRowIndex,
        cellViewerColIndex,
      );
      lobTruncated = content.truncated;
      lobSize = content.size;
      if (content.kind === 'text') {
        cellViewerKind = 'text';
        cellViewerText = content.text ?? '';
      } else {
        cellViewerKind = 'binary';
        lobMime = content.mime ?? 'application/octet-stream';
        lobBase64 = content.base64 ?? '';
        if (!lobTruncated && (lobMime === 'application/pdf' || lobMime.startsWith('image/'))) {
          const buf = base64ToBytes(lobBase64).buffer as ArrayBuffer;
          lobBlobUrl = URL.createObjectURL(new Blob([buf], { type: lobMime }));
        } else if (lobMime === 'text/plain') {
          cellViewerText = decodeText(lobBase64);
        }
      }
      cellViewerLoaded = true;
    } catch (e) {
      notify(`Failed to load content: ${String(e)}`, 'error');
    } finally {
      cellViewerLoading = false;
    }
  }

  async function saveLobToFile() {
    if (cellViewerRowIndex < 0 || cellViewerColIndex < 0) return;
    const filePath = await save({
      title: 'Save BLOB to file',
      defaultPath: `${cellViewerTitle || 'blob'}${extForMime(lobMime)}`,
    });
    if (!filePath) return;
    setBusy(true, 'Saving file…');
    try {
      const size = await saveBlobToFile(
        cellViewerServer,
        get(lastRunSql),
        cellViewerRowIndex,
        cellViewerColIndex,
        filePath,
      );
      const dir = filePath.replace(/[/\\][^/\\]+$/, '');
      notify(`Saved ${size.toLocaleString()} bytes to ${filePath}`, 'ok', dir, filePath);
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function copyCellValue() {
    navigator.clipboard.writeText(cellViewerText).then(() => notify('Copied to clipboard.', 'ok'));
  }

  // ── BLOB helpers ───────────────────────────────────────────────────
  function base64ToBytes(b64: string): Uint8Array {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }

  function decodeText(b64: string): string {
    try { return new TextDecoder().decode(base64ToBytes(b64)); } catch { return ''; }
  }

  function hexDump(b64: string, cap: number): string {
    const all = base64ToBytes(b64);
    const bytes = all.subarray(0, cap);
    let out = '';
    for (let i = 0; i < bytes.length; i += 16) {
      const chunk = bytes.subarray(i, i + 16);
      const hex = [...chunk].map(b => b.toString(16).padStart(2, '0').toUpperCase()).join(' ');
      const ascii = [...chunk].map(b => (b >= 0x20 && b < 0x7f) ? String.fromCharCode(b) : '.').join('');
      out += i.toString(16).padStart(8, '0').toUpperCase() + '  ' + hex.padEnd(47, ' ') + '  ' + ascii + '\n';
    }
    if (all.length > cap) out += `…\n(${all.length.toLocaleString()} bytes total; showing first ${cap.toLocaleString()})`;
    return out;
  }

  function extForMime(mime: string): string {
    const map: Record<string, string> = {
      'application/pdf': '.pdf', 'image/png': '.png', 'image/jpeg': '.jpg',
      'image/gif': '.gif', 'image/webp': '.webp', 'image/bmp': '.bmp', 'text/plain': '.txt',
    };
    return map[mime] ?? '.bin';
  }

  onDestroy(() => { if (lobBlobUrl) URL.revokeObjectURL(lobBlobUrl); });

  // ── Favorite ───────────────────────────────────────────────────────
  let showFavoriteModal = false;
  let favoriteModalEntry: QueryHistoryEntry | null = null;
  let favoriteModalDescription = '';

  function onFavoriteClick(entry: QueryHistoryEntry) {
    if (entry.favorite) {
      void onSetFavorite(entry.id, false, '');
    } else {
      favoriteModalEntry = entry;
      favoriteModalDescription = '';
      showFavoriteModal = true;
    }
  }

  async function onSetFavorite(id: number, favorite: boolean, description: string) {
    await setQueryFavorite(id, favorite, description).catch(() => {});
    history.set(await getQueryHistory().catch(() => []));
  }

  // ── Drag-to-reorder favorites ──────────────────────────────────────
  let dragId: number | null = null;
  let dragOverId: number | null = null;

  // The handle span is the drag SOURCE; history item divs are drop TARGETS.
  // Keeping them separate avoids the browser confusing click vs. drag on role="button".
  function onDragStart(e: DragEvent, id: number) {
    dragId = id;
    if (e.dataTransfer) {
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', String(id));
    }
  }

  function onDragOver(e: DragEvent, id: number) {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = 'move';
    dragOverId = id;
  }

  function onDrop(e: DragEvent, targetId: number) {
    e.preventDefault();
    const from = dragId;
    dragId = null; dragOverId = null;
    if (from === null || from === targetId) return;
    const favs = $history.filter(h => h.favorite);
    const others = $history.filter(h => !h.favorite);
    const fromIdx = favs.findIndex(h => h.id === from);
    const toIdx = favs.findIndex(h => h.id === targetId);
    if (fromIdx === -1 || toIdx === -1) return;
    const reordered = [...favs];
    reordered.splice(toIdx, 0, reordered.splice(fromIdx, 1)[0]);
    history.set([...reordered, ...others]);
    void reorderFavorites(reordered.map(h => h.id));
  }

  function onDragEnd() { dragId = null; dragOverId = null; }
</script>

<!-- ── Query layout: schema sidebar | main content ── -->
<div class="query-layout">
  <!-- Schema sidebar -->
  <aside class="schema-sidebar">
    <div style="display:flex;gap:4px;padding-bottom:6px;border-bottom:1px solid var(--border);margin-bottom:4px;flex-shrink:0;">
      <button class="btn-secondary" style="flex:1;font-size:11px;padding:3px 6px;" on:click={selectAll}>All</button>
      <button class="btn-secondary" style="flex:1;font-size:11px;padding:3px 6px;" on:click={selectNone}>None</button>
    </div>

    {#each [...schemaGroups.entries()] as [schema, conns]}
      <div class="schema-group">
        <div class="schema-group-header">
          <span class="schema-group-name">{schema}</span>
          <span class="schema-group-count">({conns.length})</span>
          <button class="btn-xs" on:click={() => selectSchema(schema)}>All</button>
          <button class="btn-xs" on:click={() => deselectSchema(schema)}>None</button>
        </div>
        <div class="schema-group-items">
          {#each conns as conn}
            <label class="schema-item">
              <input
                type="checkbox"
                checked={$selectedServers.has(conn.name)}
                on:change={() => toggleServer(conn.name)}
              />
              <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{conn.name}</span>
            </label>
          {/each}
        </div>
      </div>
    {:else}
      <div class="empty-state">No connections</div>
    {/each}
  </aside>

  <!-- Main area -->
  <div style="display:flex;flex-direction:column;overflow:hidden;flex:1;">
    <!-- SQL editor + toolbar -->
    <div style="padding:10px;border-bottom:1px solid var(--border);display:flex;flex-direction:column;gap:8px;flex-shrink:0;">
      <SqlEditor bind:value={$sql} height="140px" />
      <div class="row">
        <button
          class="btn-primary"
          on:click={() => void onRunQuery()}
          disabled={$busy || !$selectedServers.size}
        >▶ Run Query</button>
        <button
          class="btn-secondary"
          on:click={() => void onExport()}
          disabled={$busy || !$results.length}
        >↓ Export</button>
        <select
          bind:value={$exportMode}
          title="Result layout (grid & Excel export)"
          style="font-size:12px;padding:5px 8px;"
        >
          <option value="per-server">One tab per server</option>
          <option value="single">Single tab (all servers)</option>
        </select>
        <label
          style="display:flex;align-items:center;gap:5px;font-size:12px;color:var(--text-muted);cursor:pointer;white-space:nowrap;"
          title="Show LOB (BLOB/CLOB) content inline instead of placeholders — re-runs the query"
        >
          <input
            type="checkbox"
            bind:checked={$showLobContent}
            on:change={onToggleLobContent}
          />
          Show LOB content
        </label>
        <div class="spacer"></div>
        <button
          class="btn-secondary"
          style="font-size:12px;"
          on:click={() => historyOpen.update(v => !v)}
        >{$historyOpen ? '✕ History' : '🕐 History'}</button>
      </div>
    </div>

    <!-- Result server tabs (per-server layout only) -->
    {#if $results.length > 0 && !singleView}
      <div class="tab-strip">
        {#each $results as result}
          <button
            class="tab-btn"
            class:active={result.server_name === $activeServer}
            on:click={() => activeServer.set(result.server_name)}
          >{result.server_name}{result.error ? ' ⚠' : ''}</button>
        {/each}
      </div>
    {/if}

    <!-- Grid + history panel -->
    <div style="display:flex;flex:1;min-height:0;overflow:hidden;">
      <div style="display:flex;flex-direction:column;flex:1;min-height:0;overflow:hidden;padding:8px;">
        {#if singleView}
          {@const erroredServers = $results.filter(r => r.error).map(r => r.server_name)}
          {#if erroredServers.length}
            <div class="error-box" style="margin-bottom:8px;flex-shrink:0;">
              {erroredServers.length} server(s) errored: {erroredServers.join(', ')}
            </div>
          {/if}
        {:else if activeResult?.error}
          <div class="error-box" style="margin-bottom:8px;flex-shrink:0;">{activeResult.error}</div>
        {/if}
        <VirtualTable columns={vtCols} rows={gridRows} {onCellActivate} getCellClass={cellClass} />
        {#if singleView}
          {#if $results.length}
            <div style="flex-shrink:0;padding:3px 2px 0;font-size:11px;color:var(--text-muted,#888);">
              {gridRows.length} row{gridRows.length !== 1 ? 's' : ''} · {$results.filter(r => r.columns.length).length} server(s)
            </div>
          {/if}
        {:else if activeResult}
          <div style="flex-shrink:0;padding:3px 2px 0;font-size:11px;color:var(--text-muted,#888);">
            {gridRows.length} row{gridRows.length !== 1 ? 's' : ''}{activeResult.duration_ms !== undefined ? ` · ${fmtDuration(activeResult.duration_ms)}` : ''}
          </div>
        {/if}
      </div>

      {#if $historyOpen}
        <div class="history-panel" role="list" on:dragover|preventDefault={(e) => { if (e.dataTransfer) e.dataTransfer.dropEffect = 'move'; }}>
          <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:8px;flex-shrink:0;">
            <span style="font-size:12px;font-weight:600;color:var(--text-accent);">Query History</span>
            <button
              class="btn-danger"
              style="font-size:11px;padding:2px 6px;"
              on:click={() => void onClearHistory()}
            >Clear</button>
          </div>
          {#each $history as entry}
            <div
              class="history-item"
              class:history-item-pinned={entry.pinned && !entry.favorite}
              class:history-item-favorite={entry.favorite}
              class:history-item-drag-over={dragOverId === entry.id}
              title={entry.sql_text}
              role="button"
              tabindex="0"
              on:click={() => recallQuery(entry.sql_text)}
              on:keydown={(e) => (e.key === 'Enter' || e.key === ' ') && recallQuery(entry.sql_text)}
              on:dragover={(e) => onDragOver(e, entry.id)}
              on:dragleave={() => { dragOverId = null; }}
              on:drop|preventDefault={(e) => onDrop(e, entry.id)}
            >
              {#if entry.favorite}
                <span
                  class="drag-handle"
                  aria-hidden="true"
                  draggable="true"
                  on:dragstart={(e) => onDragStart(e, entry.id)}
                  on:dragend={onDragEnd}
                ></span>
              {/if}
              <div class="history-item-content">
                <code draggable="false">{entry.sql_text}</code>
                {#if entry.description}
                  <div class="history-desc">{entry.description}</div>
                {/if}
              </div>
              <div style="display:flex;gap:3px;flex-shrink:0;">
                <button
                  class="btn-secondary"
                  title={entry.favorite ? 'Remove from favorites' : 'Add to favorites'}
                  style="font-size:11px;padding:1px 5px;{entry.favorite ? '' : 'opacity:0.45;'}"
                  on:click|stopPropagation={() => onFavoriteClick(entry)}
                >{entry.favorite ? '⭐' : '☆'}</button>
                <button
                  class="btn-secondary"
                  title={entry.favorite ? 'Pin unavailable for favorites' : entry.pinned ? 'Unpin' : 'Pin'}
                  disabled={entry.favorite}
                  style="font-size:11px;padding:1px 5px;{entry.pinned && !entry.favorite ? '' : 'opacity:0.45;'}"
                  on:click|stopPropagation={() => void onPinHistory(entry.id, !entry.pinned)}
                >{#if entry.pinned}<span style="text-decoration:line-through;">📌</span>{:else}📌{/if}</button>
                <button
                  class="btn-danger"
                  style="font-size:10px;padding:1px 5px;{entry.pinned || entry.favorite ? 'visibility:hidden;' : ''}"
                  on:click|stopPropagation={() => void onDeleteHistory(entry.id)}
                >✕</button>
              </div>
            </div>
          {:else}
            <div class="empty-state">No history</div>
          {/each}
        </div>
      {/if}
    </div>
  </div>
</div>

{#if showFavoriteModal}
  <Modal onClose={() => (showFavoriteModal = false)}>
    <div class="modal-header">
      <span class="modal-title">Add to favorites</span>
      <button class="btn-secondary" on:click={() => (showFavoriteModal = false)}>✕</button>
    </div>
    <div class="field" style="margin-top:12px;">
      <label for="fav-desc">Description</label>
      <textarea
        id="fav-desc"
        bind:value={favoriteModalDescription}
        rows="3"
        placeholder="What does this query do?"
        style="width:100%;resize:vertical;"
      ></textarea>
    </div>
    <div class="modal-footer">
      <button
        class="btn-primary"
        on:click={() => {
          void onSetFavorite(favoriteModalEntry!.id, true, favoriteModalDescription);
          showFavoriteModal = false;
        }}
      >Save</button>
    </div>
  </Modal>
{/if}

{#if cellViewerOpen}
  <Modal width="820px" onClose={closeCellViewer}>
    <div class="modal-header">
      <span class="modal-title">{cellViewerTitle}{colType[cellViewerTitle] ? ` · ${colType[cellViewerTitle]}` : ''}</span>
      <button class="btn-secondary" on:click={closeCellViewer}>✕</button>
    </div>
    <div style="margin-top:12px;display:flex;flex-direction:column;gap:10px;">
      {#if cellViewerLoading}
        <div class="empty-state">Loading content…</div>
      {:else if cellViewerKind === 'binary'}
        {#if !cellViewerLoaded}
          <div class="empty-state">No content.</div>
        {:else if lobTruncated}
          <div style="font-size:12px;color:var(--text-muted);">
            Content is large ({lobSize.toLocaleString()} bytes shown, more exists). Use “Save to file…” to export the full BLOB.
          </div>
          <pre class="cell-viewer-pre">{hexDump(lobBase64, 4096)}</pre>
        {:else if lobMime === 'application/pdf' && lobBlobUrl}
          <iframe title="PDF preview" src={lobBlobUrl} class="cell-viewer-frame"></iframe>
        {:else if lobMime.startsWith('image/') && lobBlobUrl}
          <img src={lobBlobUrl} alt="BLOB content" class="cell-viewer-img" />
        {:else if lobMime === 'text/plain'}
          <pre class="cell-viewer-pre">{cellViewerText}</pre>
        {:else}
          <div style="font-size:12px;color:var(--text-muted);">
            {lobSize.toLocaleString()} bytes · {lobMime}. Showing a hex preview (first 4&nbsp;KB) — use “Save to file…” for the full content.
          </div>
          <pre class="cell-viewer-pre">{hexDump(lobBase64, 4096)}</pre>
        {/if}
        <div class="row" style="justify-content:flex-end;gap:8px;align-items:center;">
          <span style="flex:1;font-size:11px;color:var(--text-muted);">{lobMime}</span>
          <button class="btn-primary" disabled={$busy} on:click={() => void saveLobToFile()}>💾 Save to file…</button>
        </div>
      {:else}
        {#if lobTruncated}
          <div style="font-size:12px;color:var(--text-muted);">Text truncated to {lobSize.toLocaleString()} characters.</div>
        {/if}
        <pre class="cell-viewer-pre">{cellViewerText}</pre>
        <div class="row" style="justify-content:flex-end;">
          <button class="btn-secondary" on:click={copyCellValue}>📋 Copy</button>
        </div>
      {/if}
    </div>
  </Modal>
{/if}

<style>
  .cell-viewer-pre {
    margin: 0;
    max-height: 60vh;
    overflow: auto;
    background: var(--bg-sql);
    border: 1px solid var(--border-sql);
    border-radius: 6px;
    padding: 10px 12px;
    font-family: 'JetBrains Mono', 'Consolas', 'Courier New', monospace;
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-sql);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .cell-viewer-frame {
    width: 100%;
    height: 70vh;
    border: 1px solid var(--border-sql);
    border-radius: 6px;
    background: #fff;
  }
  .cell-viewer-img {
    max-width: 100%;
    max-height: 70vh;
    object-fit: contain;
    align-self: center;
    border: 1px solid var(--border-sql);
    border-radius: 6px;
  }
</style>

