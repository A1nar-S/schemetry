<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import VirtualTable from '../components/VirtualTable.svelte';
  import type { VCol } from '../components/VirtualTable.svelte';
  import { onMount } from 'svelte';
  import { get } from 'svelte/store';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { ConnectionRecord, McpAuditFilter, McpAuditOutcome } from '../types';
  import { getSettings, mcpAuditExportCsv, mcpAuditList, mcpAuditVerify, setOutputFolder } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';
  import {
    auditConnection,
    auditTool,
    auditOutcome,
    auditFrom,
    auditTo,
    auditEntries,
    auditTotal,
    auditSelectedId,
    auditVerification,
  } from '../stores/mcpAuditViewState';

  export let connections: ConnectionRecord[];

  const PAGE_SIZE = 500;
  const TOOLS = [
    'list_connections',
    'list_schema_objects',
    'get_object_ddl',
    'save_migration_script',
    'compare_schemas',
    'generate_fix_script',
    'query_table',
    'run_query',
  ];

  const cols: VCol[] = [
    { key: '_time',           header: 'Time',       width: 160, titleKey: '_timeRaw' },
    { key: 'tool',            header: 'Tool',       width: 160 },
    { key: 'connection_name', header: 'Connection', width: 140 },
    { key: 'client_name',     header: 'Client',     width: 120 },
    { key: 'outcome',         header: 'Outcome',    width: 90 },
    { key: '_rows',           header: 'Rows',       width: 70 },
    { key: 'bytes',           header: 'Bytes',      width: 80 },
    { key: 'duration_ms',     header: 'ms',         width: 70 },
    { key: 'reason',          header: 'Reason',     flex: 1, minWidth: 220 },
  ];

  $: rows = $auditEntries.map(e => ({
    id: e.id,
    _time: formatTime(e.timestamp),
    _timeRaw: e.timestamp,
    tool: e.tool,
    connection_name: e.connection_name,
    client_name: e.client_name,
    outcome: e.outcome,
    _rows: e.row_count ?? '',
    bytes: e.bytes,
    duration_ms: e.duration_ms,
    reason: e.reason,
  }));

  $: selected = $auditEntries.find(e => e.id === $auditSelectedId) ?? null;
  $: selectedIds = new Set<number>($auditSelectedId === null ? [] : [$auditSelectedId]);
  $: prettyArguments = selected ? prettyJson(selected.arguments) : '';

  function formatTime(ts: string): string {
    const d = new Date(ts);
    return Number.isNaN(d.getTime()) ? ts : d.toLocaleString();
  }

  function prettyJson(text: string): string {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }

  function cellClass(row: Record<string, unknown>, key: string): string {
    if (key !== 'outcome') return '';
    if (row.outcome === 'allowed') return 'vt-cell-ok';
    if (row.outcome === 'denied') return 'vt-cell-warn';
    return 'vt-cell-danger';
  }

  // Single selection: the row that was just ticked wins.
  function onSelectionChange(ids: Set<number>) {
    const current = get(auditSelectedId);
    const added = [...ids].find(id => id !== current);
    auditSelectedId.set(added ?? null);
  }

  function buildFilter(): McpAuditFilter {
    return {
      connection_id: get(auditConnection),
      tool: get(auditTool) || null,
      outcome: (get(auditOutcome) || null) as McpAuditOutcome | null,
      from: get(auditFrom) || null,
      to: get(auditTo) || null,
    };
  }

  function validateRange(): boolean {
    const from = get(auditFrom);
    const to = get(auditTo);
    if (from && to && from > to) {
      notify('"From" date must not be after "To" date.', 'error');
      return false;
    }
    return true;
  }

  async function verify() {
    try {
      auditVerification.set(await mcpAuditVerify());
    } catch (e) {
      notify(`Audit chain verification failed: ${String(e)}`, 'error');
    }
  }

  async function loadPage(append: boolean) {
    const offset = append ? get(auditEntries).length : 0;
    const page = await mcpAuditList({ ...buildFilter(), limit: PAGE_SIZE, offset });
    auditEntries.set(append ? [...get(auditEntries), ...page.entries] : page.entries);
    auditTotal.set(page.total);
    if (!append) auditSelectedId.set(null);
  }

  async function refresh() {
    if (!validateRange()) return;
    setBusy(true, 'Loading MCP audit log…');
    try {
      await loadPage(false);
      await verify();
    } catch (e) {
      notify(`Failed to load audit log: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onFilterChange() {
    if (!validateRange()) return;
    setBusy(true, 'Loading MCP audit log…');
    try {
      await loadPage(false);
    } catch (e) {
      notify(`Failed to load audit log: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function loadMore() {
    setBusy(true, 'Loading more entries…');
    try {
      await loadPage(true);
    } catch (e) {
      notify(`Failed to load audit log: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onExport() {
    if (!validateRange()) return;
    const current = await getSettings().then(s => s.output_folder).catch(() => '');
    const folder = await open({
      title: 'Export MCP Audit Log (CSV) — choose a folder',
      directory: true,
      defaultPath: current || undefined,
    }) as string | null;
    if (!folder) return;
    if (folder !== current) setOutputFolder(folder).catch(() => {});
    setBusy(true, 'Exporting audit log…');
    try {
      const path = await mcpAuditExportCsv(buildFilter(), folder);
      notify(`Audit log exported: ${path}`, 'ok', folder, path);
    } catch (e) {
      notify(`Export failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  onMount(() => {
    void refresh();
  });
</script>

<div class="audit-view">
  <!-- Chain verification -->
  {#if $auditVerification}
    {#if $auditVerification.ok}
      <div class="mcp-notice mcp-notice-ok" role="status">
        <Icon name="shield-check" size={15} />
        <span>Chain verified — {$auditVerification.entries} entries</span>
      </div>
    {:else}
      <div class="mcp-notice mcp-notice-danger" role="alert">
        <Icon name="alert-triangle" size={15} />
        <span>
          <strong>Audit log integrity problem:</strong>
          {$auditVerification.problem ?? 'the hash chain does not verify.'}
          {#if $auditVerification.first_bad_id !== null}(first bad entry: #{$auditVerification.first_bad_id}){/if}
        </span>
      </div>
    {/if}
  {/if}

  <p class="hint">
    Entries can't be edited or deleted from Schemetry. For a copy outside this machine, configure an external sink in Settings → MCP server.
  </p>

  <!-- Filters -->
  <div class="card filters">
    <div class="field">
      <span>Connection</span>
      <select bind:value={$auditConnection} on:change={() => void onFilterChange()}>
        <option value={null}>All</option>
        {#each connections as c (c.id)}
          <option value={c.id}>{c.name}</option>
        {/each}
      </select>
    </div>
    <div class="field">
      <span>Tool</span>
      <select bind:value={$auditTool} on:change={() => void onFilterChange()}>
        <option value="">All</option>
        {#each TOOLS as t}
          <option value={t}>{t}</option>
        {/each}
      </select>
    </div>
    <div class="field">
      <span>Outcome</span>
      <select bind:value={$auditOutcome} on:change={() => void onFilterChange()}>
        <option value="">All</option>
        <option value="allowed">allowed</option>
        <option value="denied">denied</option>
        <option value="error">error</option>
      </select>
    </div>
    <div class="field">
      <span>From</span>
      <input type="date" bind:value={$auditFrom} on:change={() => void onFilterChange()} />
    </div>
    <div class="field">
      <span>To</span>
      <input type="date" bind:value={$auditTo} on:change={() => void onFilterChange()} />
    </div>
    <div class="spacer"></div>
    <button class="btn-secondary" on:click={() => void refresh()} disabled={$busy}>
      <Icon name="refresh" /> Refresh
    </button>
    <button class="btn-secondary" on:click={() => void onExport()} disabled={$busy} title="Exports every entry matching the current filters">
      <Icon name="download" /> Export CSV
    </button>
  </div>

  <div class="audit-main">
    <div class="card audit-table">
      <div class="row" style="margin-bottom:8px;flex-shrink:0;">
        <span class="section-title" style="margin-bottom:0;">
          Entries ({$auditEntries.length} of {$auditTotal})
        </span>
        <div class="spacer"></div>
        {#if $auditEntries.length < $auditTotal}
          <button class="btn-secondary" style="font-size:12px;" on:click={() => void loadMore()} disabled={$busy}>
            Load more
          </button>
        {/if}
      </div>
      <VirtualTable
        columns={cols}
        {rows}
        selectable
        {selectedIds}
        getRowId={(row) => row.id as number}
        {onSelectionChange}
        getCellClass={cellClass}
      />
    </div>

    <div class="card audit-details">
      <div class="section-title">Details</div>
      {#if selected}
        {@const e = selected}
        <dl>
          <dt>Entry</dt><dd>#{e.id}</dd>
          <dt>Time (UTC)</dt><dd>{e.timestamp}</dd>
          <dt>Tool</dt><dd>{e.tool}</dd>
          <dt>Connection</dt><dd>{e.connection_name || '—'}{e.connection_id !== null ? ` (id ${e.connection_id})` : ''}</dd>
          <dt>Client</dt><dd>{e.client_name || '—'}</dd>
          <dt>Outcome</dt>
          <dd>
            <span
              class="mcp-badge"
              class:mcp-badge-ok={e.outcome === 'allowed'}
              class:mcp-badge-warn={e.outcome === 'denied'}
              class:mcp-badge-danger={e.outcome === 'error'}
            >{e.outcome}</span>
          </dd>
          <dt>Reason</dt><dd>{e.reason || '—'}</dd>
          <dt>Rows / bytes / ms</dt><dd>{e.row_count ?? '—'} / {e.bytes} / {e.duration_ms}</dd>
        </dl>
        <div class="section-title" style="margin-top:10px;">Arguments</div>
        <pre class="args">{prettyArguments}</pre>
        <div class="section-title" style="margin-top:10px;">Hash chain</div>
        <dl>
          <dt>Previous</dt><dd class="mono">{e.prev_hash || '—'}</dd>
          <dt>Hash</dt><dd class="mono">{e.hash}</dd>
        </dl>
      {:else}
        <div class="empty-state">Select an entry to see its full arguments.</div>
      {/if}
    </div>
  </div>
</div>

<style>
  .audit-view {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 10px;
    height: 100%;
    min-height: 0;
    overflow: hidden;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: 10px;
    flex-shrink: 0;
  }
  .filters select,
  .filters input {
    font-size: 13px;
    padding: 5px 8px;
  }
  .audit-main {
    display: flex;
    gap: 10px;
    flex: 1;
    min-height: 0;
  }
  .audit-table {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .audit-details {
    width: 340px;
    flex-shrink: 0;
    overflow-y: auto;
    font-size: 12px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px 10px;
    margin: 0;
  }
  dt {
    color: var(--text-muted);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
    color: var(--text-primary);
  }
  .mono,
  .args {
    font-family: 'Consolas', monospace;
    font-size: 11px;
  }
  .args {
    margin: 0;
    padding: 8px;
    max-height: 260px;
    overflow: auto;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--bg-base);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-primary);
  }
</style>
