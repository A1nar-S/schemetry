<script lang="ts">
  import { get } from 'svelte/store';
  import type { ConnectionRecord, HistoryTableIssue } from '../types';
  import SqlEditor from '../components/SqlEditor.svelte';
  import ServerSelectorModal from '../components/ServerSelectorModal.svelte';
  import HistoryNamingRulesCard from '../components/HistoryNamingRulesCard.svelte';
  import { busy, setBusy, notify } from '../stores/notification';
  import { generateHistoryFix } from '../api';
  import { selectedServers, results, activeServer } from '../stores/historyFixViewState';

  export let connections: ConnectionRecord[];

  let showSelector = false;
  let showNaming = false;

  // ── Active result ──────────────────────────────────────────────────
  $: activeResult = $results.find(r => r.server_name === $activeServer);

  // ── Group issues by history table for the active server ────────────
  $: issuesByTable = (() => {
    const m = new Map<string, HistoryTableIssue[]>();
    for (const issue of (activeResult?.issues ?? [])) {
      if (!m.has(issue.history_table)) m.set(issue.history_table, []);
      m.get(issue.history_table)!.push(issue);
    }
    return m;
  })();

  $: sortedTables = [...issuesByTable.keys()].sort();

  // ── Server selector helpers ────────────────────────────────────────
  function toggleServer(name: string) {
    selectedServers.update(s => { s.has(name) ? s.delete(name) : s.add(name); return new Set(s); });
  }
  function selectAll()  { selectedServers.set(new Set(connections.map(c => c.name))); }
  function selectNone() { selectedServers.set(new Set()); }
  function selectGroup(schema: string) {
    selectedServers.update(s => {
      connections.filter(c => (c.group_name?.trim() || 'Default') === schema).forEach(c => s.add(c.name));
      return new Set(s);
    });
  }
  function deselectGroup(schema: string) {
    selectedServers.update(s => {
      connections.filter(c => (c.group_name?.trim() || 'Default') === schema).forEach(c => s.delete(c.name));
      return new Set(s);
    });
  }

  // ── Handler ───────────────────────────────────────────────────────
  async function onGenerate() {
    const servers = get(selectedServers);
    if (!servers.size) { notify('Select at least one server.', 'error'); return; }
    showSelector = false;
    setBusy(true, 'Analysing history tables…');
    try {
      const res = await generateHistoryFix([...servers]);
      results.set(res);
      activeServer.set(res[0]?.server_name ?? '');
      const totalIssues = res.reduce((n, r) => n + r.issues.length, 0);
      const errCount = res.filter(r => r.error).length;
      if (errCount > 0) {
        notify(`Done — ${errCount} server(s) failed. ${totalIssues} issue(s) found.`, 'error');
      } else if (totalIssues === 0) {
        notify(`All history tables are in sync across ${res.length} server(s).`, 'ok');
      } else {
        notify(`Found ${totalIssues} issue(s) across ${res.length} server(s).`, 'error');
      }
    } catch (e) {
      notify(`Failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function copyFixSql() {
    if (!activeResult?.fix_sql) return;
    navigator.clipboard.writeText(activeResult.fix_sql).then(() =>
      notify('Fix SQL copied to clipboard.', 'ok'),
    );
  }

  function reset() {
    results.set([]);
    activeServer.set('');
  }
</script>

<!-- ── Server selector modal ─────────────────────────────────────── -->
{#if showSelector}
  <ServerSelectorModal
    {connections}
    selectedForFetch={$selectedServers}
    actionLabel="🔧 Generate Fix"
    onClose={() => { showSelector = false; }}
    onFetch={onGenerate}
    onToggle={toggleServer}
    onSelectAll={selectAll}
    onSelectNone={selectNone}
    onSelectSchema={selectGroup}
    onDeselectSchema={deselectGroup}
  />
{/if}

<div style="display:flex;flex-direction:column;height:100%;overflow:hidden;">

  <!-- ── Toolbar ───────────────────────────────────────────────────── -->
  <div style="padding:10px;border-bottom:1px solid var(--border);display:flex;align-items:center;gap:8px;flex-shrink:0;">
    <button
      class="btn-primary"
      disabled={$busy}
      on:click={() => { showSelector = true; }}
    >
      🔌 Select Servers{$selectedServers.size ? ` (${$selectedServers.size})` : ''}
    </button>
    {#if $results.length}
      <button class="btn-secondary" style="font-size:12px;" on:click={reset}>↺ Reset</button>
    {/if}
  </div>

  <!-- ── History table naming convention ─────────────────────────────── -->
  <div class="card" style="flex-shrink:0;margin:10px 10px 0;">
    <button
      style="display:flex;align-items:center;gap:8px;background:none;border:none;padding:0;width:100%;text-align:left;cursor:pointer;"
      on:click={() => (showNaming = !showNaming)}
    >
      <span style="font-size:11px;color:var(--text-muted);width:12px;">{showNaming ? '▼' : '▶'}</span>
      <span class="section-title" style="margin-bottom:0;">History Table Naming</span>
      <span class="hint" style="margin:0;">— rules pairing main tables with history tables</span>
    </button>
    {#if showNaming}
      <div style="margin-top:8px;">
        <HistoryNamingRulesCard />
      </div>
    {/if}
  </div>

  <!-- ── Server tabs ───────────────────────────────────────────────── -->
  {#if $results.length}
    <div class="tab-strip">
      {#each $results as r}
        <button
          class="tab-btn"
          class:active={r.server_name === $activeServer}
          on:click={() => activeServer.set(r.server_name)}
        >
          {r.server_name}
          {#if r.error}
            <span style="color:var(--text-danger,#f87171);"> ⚠</span>
          {:else if r.issues.length === 0}
            <span style="color:var(--text-ok,#34d399);font-size:11px;"> ✔</span>
          {:else}
            <span style="color:var(--text-danger,#f87171);font-size:11px;"> {r.issues.length}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}

  <!-- ── Content ───────────────────────────────────────────────────── -->
  <div style="flex:1;min-height:0;overflow:hidden;padding:10px;">
    {#if !$results.length}
      <div class="empty-state">
        Click <strong>Select Servers</strong> to pick servers and generate a fix for history-table mismatches (naming convention configurable above).
      </div>

    {:else if activeResult?.error}
      <div class="card" style="padding:16px;">
        <div style="font-size:13px;font-weight:600;color:var(--text-danger,#f87171);margin-bottom:6px;">Connection failed</div>
        <pre style="font-size:12px;color:var(--text-muted);white-space:pre-wrap;word-break:break-all;">{activeResult.error}</pre>
      </div>

    {:else if activeResult && activeResult.issues.length === 0}
      <div class="card" style="display:flex;align-items:center;gap:8px;color:var(--text-ok,#34d399);">
        <span style="font-size:18px;">✔</span>
        <span style="font-size:13px;font-weight:600;">All history tables match their base tables on {activeResult.server_name}.</span>
      </div>

    {:else if activeResult}
      <div style="display:flex;height:100%;gap:10px;overflow:hidden;">

        <!-- Left: issues grouped by history table -->
        <div class="card" style="width:320px;flex-shrink:0;display:flex;flex-direction:column;overflow:hidden;">
          <div style="font-size:12px;font-weight:600;color:var(--text-muted);margin-bottom:8px;flex-shrink:0;">
            {activeResult.issues.length} issue(s) in {issuesByTable.size} table(s)
          </div>
          <div style="overflow-y:auto;flex:1;">
            {#each sortedTables as table}
              {@const tableIssues = issuesByTable.get(table) ?? []}
              <div style="margin-bottom:10px;">
                <div style="font-size:11px;font-weight:700;color:var(--text-accent);padding:3px 0;border-bottom:1px solid var(--border-color);margin-bottom:4px;">
                  {table}
                  <span style="font-weight:400;opacity:0.7;">({tableIssues.length})</span>
                </div>
                {#each tableIssues as issue}
                  <div style="font-size:11px;padding:3px 6px;display:flex;gap:6px;align-items:baseline;">
                    {#if issue.issue_type === 'MISSING'}
                      <span style="color:var(--text-danger,#f87171);font-weight:600;white-space:nowrap;">+ ADD</span>
                      <span style="color:var(--text-primary);">{issue.column_name}</span>
                      <span style="color:var(--text-muted);font-size:10px;">{issue.main_type}</span>
                    {:else}
                      <span style="color:var(--text-warn,#fbbf24);font-weight:600;white-space:nowrap;">~ MOD</span>
                      <span style="color:var(--text-primary);">{issue.column_name}</span>
                      <span style="color:var(--text-muted);font-size:10px;">{issue.history_type} → {issue.main_type}</span>
                    {/if}
                  </div>
                {/each}
              </div>
            {/each}
          </div>
        </div>

        <!-- Right: fix SQL -->
        <div class="card" style="flex:1;display:flex;flex-direction:column;overflow:hidden;">
          <div class="row" style="margin-bottom:8px;flex-shrink:0;align-items:center;">
            <span style="font-size:13px;font-weight:600;">Fix SQL</span>
            <div class="spacer"></div>
            <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={copyFixSql}>
              📋 Copy
            </button>
          </div>
          <SqlEditor
            value={activeResult.fix_sql}
            dialect={connections.find(c => c.name === $activeServer)?.db_type ?? 'oracle'}
            readonly
            height="100%"
          />
        </div>

      </div>
    {/if}
  </div>

</div>
