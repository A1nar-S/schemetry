<script lang="ts">
  import { get } from 'svelte/store';
  import type { VCol } from '../components/VirtualTable.svelte';
  import VirtualTable from '../components/VirtualTable.svelte';
  import SqlEditor from '../components/SqlEditor.svelte';
  import DataSourcesCard from '../components/DataSourcesCard.svelte';
  import ComparisonOptionsCard from '../components/ComparisonOptionsCard.svelte';
  import ServerSelectorModal from '../components/ServerSelectorModal.svelte';
  import TableFilterRulesCard from '../components/TableFilterRulesCard.svelte';
  import { busy, setBusy, notify } from '../stores/notification';
  import { save } from '@tauri-apps/plugin-dialog';
  import type { ConnectionRecord, FixScriptResult } from '../types';
  import {
    compareDiscrepancies,
    exportCompareReport,
    fetchServers,
    generateFixScript,
    setOutputFolder,
  } from '../api';
  import {
    selectedForFetch,
    loadedServers,
    referenceServer,
    checkComments,
    checkIndexes,
    discrepancies,
    filterQuery,
    selectedIds,
    targetServer,
    generatedScripts,
    activeSqlServer,
    outputFolder,
  } from '../stores/fixViewState';

  export let connections: ConnectionRecord[];

  // ── UI-only state ────────────────
  let showSelector = false;
  let showAdvancedFilters = false;
  let vtable: VirtualTable;

  // ── VirtualTable ──────────────────────────────────────────────────
  const vtCols: VCol[] = [
    { key: '_displayId',  header: 'ID',         width: 52 },
    { key: 'difference',  header: 'Difference', width: 100 },
    { key: '_element',    header: 'Element',    width: 90 },
    { key: 'table_name',  header: 'Table',      width: 170 },
    { key: 'column_name', header: 'Column',     width: 170 },
    { key: 'server_name', header: 'Server',     width: 170 },
    { key: '_change',     header: 'Change',     flex: 1, minWidth: 260, html: true, titleKey: '_changeTitle' },
  ];


  function buildChangeTitle(details: string, refServer: string, serverName: string): string {
    const sep = ' != ';
    const idx = details.indexOf(sep);
    if (idx !== -1) {
      let ref = details.slice(0, idx);
      const actual = details.slice(idx + sep.length);
      const colonIdx = ref.indexOf(': ');
      if (colonIdx !== -1) ref = ref.slice(colonIdx + 2);
      return `${refServer}: ${ref}\n${serverName}: ${actual}`;
    }
    if (details.includes('not in server')) {
      return `${refServer}: present\n${serverName}: missing`;
    }
    if (details.includes('not in reference')) {
      return `${refServer}: missing\n${serverName}: present (extra)`;
    }
    return details;
  }

  function buildChangeHtml(details: string): string {
    const greenStyle  = 'color:var(--text-ok,#34d399);font-weight:600;white-space:nowrap;';
    const arrowStyle  = 'color:var(--text-muted);padding:0 6px;';
    const redStyle    = 'color:var(--text-danger,#f87171);font-weight:600;white-space:nowrap;';
    const yellowStyle = 'color:var(--text-warn,#fbbf24);font-weight:600;white-space:nowrap;';
    const sep = ' != ';
    const idx = details.indexOf(sep);
    if (idx !== -1) {
      let ref = details.slice(0, idx);
      const actual = details.slice(idx + sep.length);
      const colonIdx = ref.indexOf(': ');
      if (colonIdx !== -1) ref = ref.slice(colonIdx + 2);
      return `<span style="${redStyle}">${actual}</span><span style="${arrowStyle}">→</span><span style="${greenStyle}">${ref}</span>`;
    }
    if (details.includes('not in server')) {
      return `<span style="${redStyle}">missing</span><span style="${arrowStyle}">→</span><span style="${greenStyle}">present</span>`;
    }
    if (details.includes('not in reference')) {
      return `<span style="${yellowStyle}">extra</span>`;
    }
    return `<span style="${yellowStyle}">${details}</span>`;
  }

  $: filteredRows = (() => {
    let rows = $discrepancies;
    if ($targetServer) rows = rows.filter(d => d.server_name === $targetServer);
    if ($filterQuery.trim()) {
      const q = $filterQuery.toLowerCase();
      rows = rows.filter(d =>
        d.difference.toLowerCase().includes(q) ||
        d.table_name.toLowerCase().includes(q) ||
        d.column_name.toLowerCase().includes(q) ||
        d.server_name.toLowerCase().includes(q) ||
        d.details.toLowerCase().includes(q)
      );
    }
    return rows;
  })();

  $: gridRows = filteredRows.map(row => ({
    ...row,
    _discIdx: $discrepancies.indexOf(row),
    _displayId: $discrepancies.indexOf(row) + 1,
    _element: row.element,
    _change: buildChangeHtml(row.details),
    _changeTitle: buildChangeTitle(row.details, $referenceServer, row.server_name),
  }));

  const getRowId = (row: Record<string, unknown>) => row._discIdx as number;

  $: fixTargetOptions = $loadedServers.filter(s => s !== $referenceServer);

  // ── Server selector helpers ────────────────────────────────────────
  function toggleForFetch(name: string) {
    selectedForFetch.update(s => { s.has(name) ? s.delete(name) : s.add(name); return new Set(s); });
  }
  function selectSchema(schema: string) {
    selectedForFetch.update(s => {
      connections.filter(c => (c.group_name?.trim() || 'Default') === schema).forEach(c => s.add(c.name));
      return new Set(s);
    });
  }
  function deselectSchema(schema: string) {
    selectedForFetch.update(s => {
      connections.filter(c => (c.group_name?.trim() || 'Default') === schema).forEach(c => s.delete(c.name));
      return new Set(s);
    });
  }

  // ── Handlers ──────────────────────────────────────────────────────
  async function onFetch() {
    const selected = get(selectedForFetch);
    if (!selected.size) { notify('Select at least one server to fetch.', 'error'); return; }
    setBusy(true, 'Fetching schema metadata…');
    try {
      const res = await fetchServers([...selected]);
      loadedServers.set(res.loaded_servers);
      const loaded = res.loaded_servers;
      if (!loaded.includes(get(referenceServer))) referenceServer.set(loaded[0] ?? '');
      targetServer.set('');
      const errMsg = res.errors.map(e => `${e.server}: ${e.error}`).join(' | ');
      notify(
        res.errors.length ? `Fetched ${loaded.length} server(s). Errors: ${errMsg}` : `Fetched ${loaded.length} server(s).`,
        res.errors.length ? 'error' : 'ok',
      );
      showSelector = false;
    } catch (e) {
      notify(`Fetch failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onCompare() {
    const ref = get(referenceServer);
    if (!ref) { notify('Select a reference server.', 'error'); return; }
    setBusy(true, 'Comparing schemas…');
    try {
      const result = await compareDiscrepancies({
        reference_server: ref,
        check_comments: get(checkComments),
        check_indexes: get(checkIndexes),
      });
      discrepancies.set(result);
      selectedIds.set(new Set());
      generatedScripts.set(new Map());
      activeSqlServer.set('');
      notify(`Comparison complete. ${result.length} discrepancy(s) found.`, 'ok');
    } catch (e) {
      notify(`Comparison failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onGenerateFix() {
    const ids = get(selectedIds);
    const ref = get(referenceServer);
    if (!ids.size) { notify('Select at least one discrepancy to fix.', 'error'); return; }
    if (!ref) { notify('Select a reference server.', 'error'); return; }
    setBusy(true, 'Generating fix script…');
    try {
      const allDiscs = get(discrepancies);
      const byServer = new Map<string, number[]>();
      for (const idx of ids) {
        const srv = allDiscs[idx].server_name;
        if (!byServer.has(srv)) byServer.set(srv, []);
        byServer.get(srv)!.push(idx);
      }
      const scripts = new Map<string, FixScriptResult>();
      for (const [server, serverIds] of byServer) {
        const result = await generateFixScript({
          discrepancies: allDiscs,
          selected_ids: serverIds,
          reference_server: ref,
        });
        scripts.set(server, result);
      }
      generatedScripts.set(scripts);
      activeSqlServer.set([...scripts.keys()][0] ?? '');
      const total   = [...scripts.values()].reduce((n, r) => n + r.generated_count, 0);
      const skipped = [...scripts.values()].reduce((n, r) => n + r.skipped_count, 0);
      notify(`Fix script generated: ${total} statement(s), ${skipped} skipped.`, 'ok');
    } catch (e) {
      notify(`Fix generation failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onExportReport() {
    if (!get(discrepancies).length) { notify('No discrepancies to export.', 'error'); return; }
    const lastDir = get(outputFolder);
    const filePath = await save({
      title: 'Export Discrepancy Report',
      defaultPath: lastDir ? `${lastDir}\\discrepancy_report.xlsx` : 'discrepancy_report.xlsx',
      filters: [{ name: 'Excel', extensions: ['xlsx'] }],
    });
    if (!filePath) return;
    const dir = filePath.replace(/[\/\\][^\/\\]+$/, '');
    if (dir !== lastDir) {
      outputFolder.set(dir);
      setOutputFolder(dir).catch(() => {});
    }
    setBusy(true, 'Exporting report…');
    try {
      const [, xlsx] = await exportCompareReport(get(discrepancies), dir);
      notify(`Report saved: ${xlsx}`, 'ok', dir, xlsx);
    } catch (e) {
      notify(`Export failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function copyToClipboard() {
    const script = get(generatedScripts).get(get(activeSqlServer))?.script ?? '';
    navigator.clipboard.writeText(script).then(() => notify('Fix script copied to clipboard.', 'ok'));
  }

  function onSelectionChange(ids: Set<number>) {
    selectedIds.set(ids);
  }

  function selectAllRows() {
    selectedIds.set(new Set(filteredRows.map(row => $discrepancies.indexOf(row))));
  }

  function clearSelection() {
    vtable?.clearSelection();
  }
</script>

<!-- ── View wrapper ─────────────────────────────────────────────── -->
<div style="display:flex;flex-direction:column;height:100%;overflow:hidden;padding:10px;gap:10px;">

  <!-- Row 1: Data Sources card + Options card -->
  <div class="compare-grid" style="flex-shrink:0;">
    <DataSourcesCard
      loadedServers={$loadedServers}
      onOpenSelector={() => (showSelector = true)}
    />
    <ComparisonOptionsCard
      loadedServers={$loadedServers}
      referenceServer={$referenceServer}
      checkComments={$checkComments}
      checkIndexes={$checkIndexes}
      onReferenceChange={(s) => referenceServer.set(s)}
      onToggleComments={(v) => checkComments.set(v)}
      onToggleIndexes={(v) => checkIndexes.set(v)}
      onRunComparison={() => void onCompare()}
    />
  </div>

  <!-- Advanced Filters (table name include/exclude rules) -->
  <div class="card" style="flex-shrink:0;">
    <button
      style="display:flex;align-items:center;gap:8px;background:none;border:none;padding:0;width:100%;text-align:left;cursor:pointer;"
      on:click={() => (showAdvancedFilters = !showAdvancedFilters)}
    >
      <span style="font-size:11px;color:var(--text-muted);width:12px;">{showAdvancedFilters ? '▼' : '▶'}</span>
      <span class="section-title" style="margin-bottom:0;">Advanced Filters</span>
      <span class="hint" style="margin:0;">— which tables get fetched from Oracle</span>
    </button>
    {#if showAdvancedFilters}
      <div style="margin-top:8px;">
        <TableFilterRulesCard />
      </div>
    {/if}
  </div>

  <!-- Server selector modal -->
  {#if showSelector}
    <ServerSelectorModal
      {connections}
      selectedForFetch={$selectedForFetch}
      onClose={() => (showSelector = false)}
      onFetch={() => void onFetch()}
      onToggle={toggleForFetch}
      onSelectAll={() => selectedForFetch.set(new Set(connections.map(c => c.name)))}
      onSelectNone={() => selectedForFetch.set(new Set())}
      onSelectSchema={selectSchema}
      onDeselectSchema={deselectSchema}
    />
  {/if}

  <!-- Discrepancy grid -->
  <div class="card" style="display:flex;flex-direction:column;flex:1;min-height:200px;overflow:hidden;">
    <div class="row" style="margin-bottom:8px;flex-shrink:0;">
      <span class="section-title" style="margin-bottom:0;">
        Discrepancies ({filteredRows.length}{$filterQuery || $targetServer ? ` of ${$discrepancies.length}` : ''})
      </span>
      <select
        bind:value={$targetServer}
        style="max-width:180px;font-size:12px;padding:4px 8px;"
        disabled={!fixTargetOptions.length}
      >
        <option value="">All servers</option>
        {#each fixTargetOptions as s}
          <option value={s}>{s}</option>
        {/each}
      </select>
      <input
        style="max-width:200px;font-size:12px;padding:4px 8px;"
        placeholder="Filter…"
        bind:value={$filterQuery}
      />
      <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={selectAllRows}>Select All</button>
      <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={clearSelection}>Clear</button>
      <div class="spacer"></div>
      <button
        class="btn-secondary"
        style="font-size:11px;padding:3px 8px;"
        on:click={() => void onExportReport()}
        disabled={$busy || !$discrepancies.length}
      >↓ Export</button>
    </div>
    <VirtualTable
      bind:this={vtable}
      columns={vtCols}
      rows={gridRows}
      selectable
      selectedIds={$selectedIds}
      {getRowId}
      onSelectionChange={onSelectionChange}
      getCellClass={(row, key) => {
        if (key !== 'difference') return '';
        if (row.difference === 'MISSING') return 'vt-cell-missing';
        if (row.difference === 'DIFFERENT') return 'vt-cell-different';
        return '';
      }}
    />
  </div>

  <!-- Fix section -->
  <div class="card" style="flex-shrink:0;">
    <div class="row" style="margin-bottom:8px;align-items:center;">
      <span class="section-title" style="margin-bottom:0;">Fix Script</span>
      <button
        class="btn-primary"
        on:click={() => void onGenerateFix()}
        disabled={$busy || !$selectedIds.size || !$referenceServer}
      >🔧 Generate Fix ({$selectedIds.size} selected)</button>
      {#if $generatedScripts.size}
        <button class="btn-secondary" on:click={copyToClipboard}>📋 Copy</button>
        <button class="btn-secondary" style="color:#f87171;" on:click={() => { generatedScripts.set(new Map()); activeSqlServer.set(''); }}>Clear</button>
      {/if}
    </div>

    {#if $generatedScripts.size}
      {#if $generatedScripts.size > 1}
        <div class="tab-strip" style="margin-bottom:6px;">
          {#each [...$generatedScripts.entries()] as [server, res]}
            <button
              class="tab-btn"
              class:active={server === $activeSqlServer}
              on:click={() => activeSqlServer.set(server)}
            >
              {server}
              <span style="font-size:11px;opacity:0.75;"> ({res.generated_count})</span>
            </button>
          {/each}
        </div>
      {/if}
      <SqlEditor
        value={$generatedScripts.get($activeSqlServer)?.script ?? ''}
        dialect={connections.find(c => c.name === $activeSqlServer)?.db_type ?? 'oracle'}
        readonly
        height="260px"
      />
    {:else}
      <div class="empty-state">
        {#if !$discrepancies.length}
          Run a comparison first, then select rows to generate a fix script.
        {:else}
          Select discrepancy rows above and click Generate Fix.
        {/if}
      </div>
    {/if}
  </div>
</div>
