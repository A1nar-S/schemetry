<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import { onMount } from 'svelte';
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
    fetchCompletionMetadata,
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
  import LobViewer from '../components/LobViewer.svelte';
  import { toCompletionSchema } from '../sqlCompletion';
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
    exportFormat,
    lastRunSql,
    showLobContent,
    expandedGroups,
    completionCache,
    completionServerPick,
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
    selectedServers.update(s => { (schemaGroups.get(schema) ?? []).forEach(c => s.add(c.id)); return new Set(s); });
  }
  function deselectSchema(schema: string) {
    selectedServers.update(s => { (schemaGroups.get(schema) ?? []).forEach(c => s.delete(c.id)); return new Set(s); });
  }
  function clearSelection() {
    selectedServers.set(new Set());
  }
  function toggleServer(id: number) {
    selectedServers.update(s => { s.has(id) ? s.delete(id) : s.add(id); return new Set(s); });
  }
  function toggleGroupCollapsed(schema: string) {
    expandedGroups.update(s => { s.has(schema) ? s.delete(schema) : s.add(schema); return new Set(s); });
  }
  // Dialect for the editor: the active/first-selected server's engine. Mixed-engine
  // selections just highlight for whichever one happens to be picked first.
  // The server the editor works against (dialect + autocomplete): the one picked in
  // the toolbar, else the active results tab, else the first selected connection.
  $: selectedConns = connections.filter(c => $selectedServers.has(c.id));
  $: if ($completionServerPick !== null && !$selectedServers.has($completionServerPick)) {
    completionServerPick.set(null);
  }
  $: autoEditorServer =
    connections.find(c => c.id === $activeServer)
    ?? selectedConns[0];
  $: editorServer = connections.find(c => c.id === $completionServerPick) ?? autoEditorServer;
  $: queryDialect = editorServer?.db_type ?? 'oracle';

  // ── Autocomplete ───────────────────────────────────────────────────
  // Table/column names are fetched only when the editor is focused — not on selection
  // or tab changes — and cached per server for the session.
  let completionLoadingId: number | null = null;
  let completionError = '';

  function onEditorFocus() {
    if (editorServer) void loadCompletion(editorServer.id);
  }
  $: completionMeta = editorServer ? $completionCache[editorServer.id] : undefined;
  // A different server's error shouldn't stick around.
  $: editorServerId = editorServer?.id;
  $: editorServerId, (completionError = '');
  $: completionSchema = completionMeta ? toCompletionSchema(completionMeta) : undefined;

  async function loadCompletion(serverId: number, force = false) {
    if (!force && (get(completionCache)[serverId] || completionLoadingId === serverId)) return;
    completionLoadingId = serverId;
    completionError = '';
    try {
      const meta = await fetchCompletionMetadata(serverId);
      completionCache.update(c => ({ ...c, [serverId]: meta }));
    } catch (e) {
      if (editorServer?.id === serverId) completionError = String(e);
    } finally {
      if (completionLoadingId === serverId) completionLoadingId = null;
    }
  }

  // ── VirtualTable data ─────────────────────────────────────────────
  $: activeResult = $results.find(r => r.server_id === $activeServer);
  $: singleView = $exportMode === 'single';

  // The result whose columns/types define the grid: active server, or the first
  // server with data when showing all servers in one combined table.
  $: baseResult = singleView ? $results.find(r => r.columns.length) : activeResult;

  // Map of column name → Oracle type label, used to flag openable LOB cells.
  $: colType = baseResult
    ? Object.fromEntries(baseResult.columns.map((name, i) => [name, baseResult!.column_types?.[i] ?? '']))
    : {};

  // 'binary' = BLOB-family (rich viewer), 'text' = CLOB-family (text viewer), null = plain.
  // Covers both engines: Oracle's LOB types and Postgres's `bytea` (its only real
  // binary-LOB-like type — Postgres text/varchar are already fully materialized).
  function lobKindOf(t: string | undefined): 'binary' | 'text' | null {
    if (t === 'BLOB' || t === 'BFILE' || t === 'LONG RAW' || t === 'BYTEA') return 'binary';
    if (t === 'CLOB' || t === 'NCLOB' || t === 'LONG') return 'text';
    return null;
  }
  function lobKind(colKey: string): 'binary' | 'text' | null {
    return lobKindOf(colType[colKey]);
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
      const out: Record<string, string | number | null> = {
        __rowIndex: idx,
        __server: r.server_name,
        __serverId: r.server_id,
      };
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
      activeServer.set(queryResults[0]?.server_id ?? null);
      history.set(await getQueryHistory());
      if (queryResults.some(r => r.error === 'Cancelled by user.')) {
        notify('Query cancelled.', 'error');
      } else {
        notify(`Query executed on ${queryResults.length} server(s).`, 'ok');
      }
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
    // The backend picks the format from the extension; the last-used one is offered first.
    const excelFilter = { name: 'Excel', extensions: ['xlsx'] };
    const csvFilter = { name: 'CSV (comma-separated)', extensions: ['csv'] };
    const format = get(exportFormat);
    const fileName = `query_export.${format}`;
    const filePath = await save({
      title: 'Export Query Results',
      defaultPath: lastDir ? `${lastDir}\\${fileName}` : fileName,
      filters: format === 'csv' ? [csvFilter, excelFilter] : [excelFilter, csvFilter],
    });
    if (!filePath) return;
    exportFormat.set(/\.csv$/i.test(filePath) ? 'csv' : 'xlsx');
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

  // Clearing keeps pinned and favorite queries.
  let showClearHistoryModal = false;
  $: clearableCount = $history.filter(h => !h.pinned && !h.favorite).length;

  async function onClearHistory() {
    showClearHistoryModal = false;
    try {
      await clearQueryHistory();
      notify('Query history cleared.', 'ok');
    } catch (e) {
      notify(`Couldn't clear history: ${String(e)}`, 'error');
    }
    history.set(await getQueryHistory().catch(() => []));
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
  let cellViewerType = '';
  let cellViewerIsLob = false;
  let cellViewerRowIndex = -1;
  let cellViewerColIndex = -1;
  let cellViewerResultId: number | null = null;
  let cellViewerLoading = false;
  let cellViewerText: string | null = null;
  let cellViewerBytes: Uint8Array | null = null;
  let cellViewerSize = 0;
  let cellViewerTruncated = false;
  let cellViewerSaving = false;
  // Discards stale responses for a previously opened cell.
  let lobRequestSeq = 0;

  function closeCellViewer() {
    lobRequestSeq++;
    cellViewerOpen = false;
    cellViewerText = null;
    cellViewerBytes = null;
  }

  function onCellActivate(row: Record<string, unknown>, colKey: string) {
    lobRequestSeq++;
    cellViewerTitle = colKey;
    cellViewerText = null;
    cellViewerBytes = null;
    cellViewerTruncated = false;
    cellViewerRowIndex = typeof row.__rowIndex === 'number' ? row.__rowIndex : -1;
    const serverId = typeof row.__serverId === 'number' ? row.__serverId : get(activeServer);
    // Use the row's own server result — column order can differ between servers.
    const rowResult = get(results).find(r => r.server_id === serverId) ?? baseResult;
    cellViewerResultId = rowResult?.result_id ?? null;
    cellViewerColIndex = rowResult ? rowResult.columns.indexOf(colKey) : -1;
    cellViewerType = rowResult?.column_types?.[cellViewerColIndex] ?? colType[colKey] ?? '';
    cellViewerIsLob = lobKindOf(cellViewerType) !== null;
    cellViewerOpen = true;

    if (!cellViewerIsLob) {
      const value = row[colKey];
      cellViewerText = value == null ? '' : String(value);
      cellViewerSize = cellViewerText.length;
      return;
    }
    void loadLobContent();
  }

  async function loadLobContent() {
    if (cellViewerRowIndex < 0 || cellViewerColIndex < 0 || cellViewerResultId === null) return;
    const seq = lobRequestSeq;
    cellViewerLoading = true;
    try {
      const content = await fetchLobContent(cellViewerResultId, cellViewerRowIndex, cellViewerColIndex);
      if (seq !== lobRequestSeq) return;
      cellViewerTruncated = content.truncated;
      cellViewerSize = content.size;
      if (content.kind === 'text') cellViewerText = content.text ?? '';
      else cellViewerBytes = base64ToBytes(content.base64 ?? '');
    } catch (e) {
      if (seq === lobRequestSeq) notify(`Failed to load content: ${String(e)}`, 'error');
    } finally {
      if (seq === lobRequestSeq) cellViewerLoading = false;
    }
  }

  async function saveLobToFile(ext: string) {
    if (cellViewerRowIndex < 0 || cellViewerColIndex < 0 || cellViewerResultId === null) return;
    const filePath = await save({
      title: `Save ${cellViewerType || 'LOB'} to file`,
      defaultPath: `${cellViewerTitle || 'lob'}${ext}`,
    });
    if (!filePath) return;
    cellViewerSaving = true;
    try {
      const size = await saveBlobToFile(cellViewerResultId, cellViewerRowIndex, cellViewerColIndex, filePath);
      const dir = filePath.replace(/[/\\][^/\\]+$/, '');
      notify(`Saved ${size.toLocaleString()} bytes to ${filePath}`, 'ok', dir, filePath);
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      cellViewerSaving = false;
    }
  }

  function base64ToBytes(b64: string): Uint8Array {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }

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
    {#if schemaGroups.size}
      <div class="schema-selection-bar">
        <span>{$selectedServers.size} selected</span>
        <button class="btn-xs" disabled={!$selectedServers.size} on:click={clearSelection}>None</button>
      </div>
    {/if}
    {#each [...schemaGroups.entries()] as [schema, conns]}
      {@const collapsed = !$expandedGroups.has(schema)}
      {@const selectedCount = conns.filter(c => $selectedServers.has(c.id)).length}
      <div class="schema-group" class:has-selection={selectedCount > 0}>
        <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
        <div class="schema-group-header" on:click={() => toggleGroupCollapsed(schema)}>
          <div class="schema-group-title">
            <span class="schema-group-chevron" class:collapsed>▾</span>
            <span class="schema-group-name" title={schema}>{schema}</span>
          </div>
          <div class="schema-group-meta">
            {#if selectedCount}
              <span class="schema-group-count schema-group-count-selected">{selectedCount}/{conns.length} selected</span>
            {:else}
              <span class="schema-group-count">{conns.length} connection{conns.length === 1 ? '' : 's'}</span>
            {/if}
            <!-- svelte-ignore a11y-click-events-have-key-events a11y-no-static-element-interactions -->
            <div class="schema-group-actions" on:click|stopPropagation>
              <button title="Select all in {schema}" on:click={() => selectSchema(schema)}>All</button>
              <button title="Deselect all in {schema}" on:click={() => deselectSchema(schema)}>None</button>
            </div>
          </div>
        </div>
        {#if !collapsed}
          <div class="schema-group-items">
            {#each conns as conn}
              <label class="schema-item">
                <input
                  type="checkbox"
                  checked={$selectedServers.has(conn.id)}
                  on:change={() => toggleServer(conn.id)}
                />
                <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{conn.name}</span>
              </label>
            {/each}
          </div>
        {/if}
      </div>
    {:else}
      <div class="empty-state">No connections</div>
    {/each}
  </aside>

  <!-- Main area -->
  <div style="display:flex;flex-direction:column;overflow:hidden;flex:1;">
    <!-- SQL editor + toolbar -->
    <div style="padding:10px;border-bottom:1px solid var(--border);display:flex;flex-direction:column;gap:8px;flex-shrink:0;">
      <SqlEditor
        bind:value={$sql}
        dialect={queryDialect}
        schema={completionSchema}
        onFocus={onEditorFocus}
        defaultSchema={completionMeta?.schema}
        height="140px"
      />
      <div class="row">
        <button
          class="btn-primary"
          on:click={() => void onRunQuery()}
          disabled={$busy || !$selectedServers.size}
        ><Icon name="play" /> Run Query</button>
        <button
          class="btn-secondary"
          on:click={() => void onExport()}
          disabled={$busy || !$results.length}
        ><Icon name="download" /> Export</button>
        <select
          bind:value={$exportMode}
          title="Result layout (grid & Excel export; CSV export is always a single table)"
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
        {#if editorServer}
          <span
            class="completion-status"
            class:completion-status-error={completionError && !completionMeta}
            title={completionError || 'Server used for autocomplete and SQL highlighting'}
          >
            Autocomplete:
            {#if selectedConns.length > 1}
              <select bind:value={$completionServerPick} class="completion-server">
                <option value={null}>Auto{autoEditorServer ? ` (${autoEditorServer.name})` : ''}</option>
                {#each selectedConns as conn}
                  <option value={conn.id}>{conn.name}{conn.group_name ? ` · ${conn.group_name}` : ''}</option>
                {/each}
              </select>
            {:else}
              {editorServer.name} ·
            {/if}
            {#if completionLoadingId === editorServer.id}
              loading…
            {:else if completionMeta}
              {completionMeta.relations.length} tables
            {:else if completionError}
              unavailable
            {:else}
              loads when you edit
            {/if}
            <button
              class="btn-xs"
              title="Reload table and column names"
              disabled={completionLoadingId === editorServer.id}
              on:click={() => editorServer && void loadCompletion(editorServer.id, true)}
            ><Icon name="refresh" size={12} /></button>
          </span>
        {/if}
        <button
          class="btn-secondary"
          style="font-size:12px;"
          on:click={() => historyOpen.update(v => !v)}
        ><Icon name={$historyOpen ? 'x' : 'clock'} /> History</button>
      </div>
    </div>

    <!-- Result server tabs (per-server layout only) -->
    {#if $results.length > 0 && !singleView}
      <div class="tab-strip">
        {#each $results as result}
          <button
            class="tab-btn"
            class:active={result.server_id === $activeServer}
            on:click={() => activeServer.set(result.server_id)}
          >{result.server_name}{#if result.error} <Icon name="alert-triangle" size={12} />{/if}</button>
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
        <VirtualTable columns={vtCols} rows={gridRows} {onCellActivate} getCellClass={cellClass} filterable />
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
              disabled={!clearableCount}
              title={clearableCount ? 'Clear history (pinned and favorite queries are kept)' : 'Nothing to clear'}
              on:click={() => (showClearHistoryModal = true)}
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
                ><Icon name="star" size={12} filled={entry.favorite} /></button>
                <button
                  class="btn-secondary"
                  title={entry.favorite ? 'Pin unavailable for favorites' : entry.pinned ? 'Unpin' : 'Pin'}
                  disabled={entry.favorite}
                  style="font-size:11px;padding:1px 5px;{entry.pinned && !entry.favorite ? '' : 'opacity:0.45;'}"
                  on:click|stopPropagation={() => void onPinHistory(entry.id, !entry.pinned)}
                ><Icon name={entry.pinned ? 'pin-off' : 'pin'} size={12} /></button>
                <button
                  class="btn-danger"
                  style="font-size:10px;padding:1px 5px;{entry.pinned || entry.favorite ? 'visibility:hidden;' : ''}"
                  on:click|stopPropagation={() => void onDeleteHistory(entry.id)}
                ><Icon name="x" size={11} /></button>
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

{#if showClearHistoryModal}
  <Modal onClose={() => (showClearHistoryModal = false)}>
    <div class="modal-header">
      <span class="modal-title">Clear query history?</span>
      <button class="btn-secondary" on:click={() => (showClearHistoryModal = false)} title="Close"><Icon name="x" /></button>
    </div>
    <p style="margin:12px 0 0;font-size:13px;color:var(--text-primary);">
      This permanently deletes {clearableCount} {clearableCount === 1 ? 'query' : 'queries'} from history.
      Pinned and favorite queries are kept.
    </p>
    <div class="modal-footer">
      <button class="btn-secondary" on:click={() => (showClearHistoryModal = false)}>Cancel</button>
      <button class="btn-danger" on:click={() => void onClearHistory()}>Clear history</button>
    </div>
  </Modal>
{/if}

{#if showFavoriteModal}
  <Modal onClose={() => (showFavoriteModal = false)}>
    <div class="modal-header">
      <span class="modal-title">Add to favorites</span>
      <button class="btn-secondary" on:click={() => (showFavoriteModal = false)} title="Close"><Icon name="x" /></button>
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
  <LobViewer
    title={cellViewerTitle}
    typeLabel={cellViewerType}
    loading={cellViewerLoading}
    text={cellViewerText}
    bytes={cellViewerBytes}
    size={cellViewerSize}
    truncated={cellViewerTruncated}
    onSave={cellViewerIsLob ? (ext) => void saveLobToFile(ext) : undefined}
    saving={cellViewerSaving}
    onClose={closeCellViewer}
  />
{/if}
