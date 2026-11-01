<script lang="ts">
  import Icon from '../components/Icon.svelte';
  import Modal from '../components/Modal.svelte';
  import SqlEditor from '../components/SqlEditor.svelte';
  import { get } from 'svelte/store';
  import type { ConnectionRecord } from '../types';
  import { applyScript } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';
  import {
    applySelectedServers as selected,
    applyScriptText as script,
    applyResults as results,
    applyLastRun as lastRun,
  } from '../stores/applyViewState';

  export let connections: ConnectionRecord[];

  let confirming = false;
  let acknowledged = false;

  $: groups = buildGroups(connections);
  $: selectedConns = connections.filter(c => $selected.has(c.id));
  // Drop ids of connections that no longer exist so counts and the confirm dialog agree.
  $: if ($selected.size !== selectedConns.length) selected.set(new Set(selectedConns.map(c => c.id)));
  $: dialect = selectedConns[0]?.db_type ?? 'oracle';
  $: mixedEngines = new Set(selectedConns.map(c => c.db_type)).size > 1;
  $: lineCount = $script.trim() ? $script.trim().split('\n').length : 0;
  // Several servers: make the user acknowledge it explicitly, not just click through.
  $: needsAck = selectedConns.length > 1;

  function buildGroups(conns: ConnectionRecord[]): Map<string, ConnectionRecord[]> {
    const map = new Map<string, ConnectionRecord[]>();
    for (const c of conns) {
      const key = c.group_name?.trim() || 'Default';
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(c);
    }
    return map;
  }

  function toggle(id: number) {
    selected.update(s => { s.has(id) ? s.delete(id) : s.add(id); return new Set(s); });
  }
  function setGroup(name: string, on: boolean) {
    selected.update(s => {
      (groups.get(name) ?? []).forEach(c => (on ? s.add(c.id) : s.delete(c.id)));
      return new Set(s);
    });
  }

  function askToRun() {
    if (!$script.trim()) { notify('The script is empty.', 'error'); return; }
    if (!$selected.size) { notify('Select at least one server.', 'error'); return; }
    if (mixedEngines) { notify('Select servers of one engine only (Oracle or PostgreSQL).', 'error'); return; }
    acknowledged = false;
    confirming = true;
  }

  async function run() {
    confirming = false;
    const ids = [...get(selected)];
    setBusy(true, 'Applying script…');
    try {
      const res = await applyScript(get(script), ids);
      results.set(res);
      lastRun.set({ at: new Date().toLocaleTimeString(), servers: ids.length });
      const failed = res.filter(r => r.error).length;
      const compileIssues = res.reduce((n, r) => n + r.compile_errors.length, 0);
      if (failed) notify(`${failed} of ${res.length} server(s) failed.`, 'error');
      else if (compileIssues) notify('Applied, but some objects have compile errors.', 'error');
      else notify(`Applied on ${res.length} server(s).`, 'ok');
    } catch (e) {
      notify(String(e), 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="query-layout">
  <aside class="schema-sidebar">
    {#if groups.size}
      <div class="schema-selection-bar">
        <span>{$selected.size} selected</span>
        <button class="btn-xs" disabled={!$selected.size} on:click={() => selected.set(new Set())}>None</button>
      </div>
    {/if}
    {#each [...groups.entries()] as [name, conns]}
      <div class="schema-group" class:has-selection={conns.some(c => $selected.has(c.id))}>
        <div class="schema-group-header" style="cursor:default;">
          <div class="schema-group-title"><span class="schema-group-name" title={name}>{name}</span></div>
          <div class="schema-group-meta">
            <div class="schema-group-actions">
              <button on:click={() => setGroup(name, true)}>All</button>
              <button on:click={() => setGroup(name, false)}>None</button>
            </div>
          </div>
        </div>
        <div class="schema-group-items">
          {#each conns as conn}
            <label class="schema-item">
              <input type="checkbox" checked={$selected.has(conn.id)} on:change={() => toggle(conn.id)} />
              <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">{conn.name}</span>
            </label>
          {/each}
        </div>
      </div>
    {:else}
      <div class="empty-state">No connections</div>
    {/each}
  </aside>

  <div style="display:flex;flex-direction:column;overflow:hidden;flex:1;">
    <div style="padding:10px;border-bottom:1px solid var(--border);display:flex;flex-direction:column;gap:8px;flex-shrink:0;">
      <div class="hint" style="margin:0;">
        Runs on every selected server with that connection's own account. Oracle: statements run one by one and each
        is committed; the first error stops the script. PostgreSQL: one transaction, so any error rolls everything
        back. Put a <code>/</code> line after each procedure, function, package or trigger.
      </div>
      <SqlEditor bind:value={$script} {dialect} height="240px" />
      <div class="row">
        <button class="btn-primary" on:click={askToRun} disabled={$busy || !$selected.size}>
          <Icon name="play" /> Apply script
        </button>
        <button class="btn-secondary" on:click={() => { script.set(''); results.set([]); lastRun.set(null); }} disabled={$busy || (!$script && !$results.length)}>
          <Icon name="x" /> Clear
        </button>
        <div class="spacer"></div>
        <span style="font-size:12px;color:var(--text-muted);">
          {lineCount} line{lineCount === 1 ? '' : 's'} · {$selected.size} server{$selected.size === 1 ? '' : 's'}
        </span>
      </div>
    </div>

    <div style="flex:1;overflow:auto;padding:10px;display:flex;flex-direction:column;gap:10px;">
      {#if $lastRun}
        <div style="font-size:12px;color:var(--text-muted);">Last run at {$lastRun.at} on {$lastRun.servers} server(s)</div>
      {/if}
      {#each $results as r (r.server_id)}
        <div class="card" style="padding:10px 12px;">
          <div class="row" style="gap:8px;">
            <Icon name={r.error ? 'alert-triangle' : 'circle-check'} size={16} />
            <strong>{r.server_name}</strong>
            <span style="font-size:12px;color:var(--text-muted);">
              {r.executed}/{r.statement_count} statement{r.statement_count === 1 ? '' : 's'} · {r.duration_ms} ms
            </span>
          </div>
          {#if r.error}
            <pre style="margin:8px 0 0;white-space:pre-wrap;color:var(--error-color);font-size:12px;">{r.error}</pre>
          {:else if !r.compile_errors.length}
            <div style="margin-top:6px;font-size:12px;color:var(--text-muted);">Applied without errors.</div>
          {/if}
          {#if r.compile_errors.length}
            <table style="margin-top:8px;font-size:12px;width:100%;border-collapse:collapse;">
              <thead>
                <tr style="text-align:left;color:var(--text-muted);">
                  <th>Object</th><th>Severity</th><th>Line:Col</th><th>Message</th>
                </tr>
              </thead>
              <tbody>
                {#each r.compile_errors as e}
                  <tr>
                    <td>{e.object_type} {e.object}</td>
                    <td>{e.severity}</td>
                    <td>{e.line}:{e.position}</td>
                    <td style="white-space:pre-wrap;">{e.text}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          {/if}
        </div>
      {:else}
        <div class="empty-state">Paste a DDL script, pick servers, and apply it. Nothing runs until you confirm.</div>
      {/each}
    </div>
  </div>
</div>

{#if confirming}
  <Modal width="560px" onClose={() => (confirming = false)}>
    <div class="modal-header">
      <span class="modal-title">Apply this script?</span>
      <button class="btn-secondary" on:click={() => (confirming = false)} title="Close"><Icon name="x" /></button>
    </div>
    <p style="margin:12px 0 4px;font-size:13px;color:var(--text-primary);">
      The {lineCount}-line script will be executed on {selectedConns.length}
      server{selectedConns.length === 1 ? '' : 's'}. Changes to the database cannot be undone from here.
    </p>
    <ul style="margin:6px 0 10px;padding-left:18px;font-size:13px;max-height:180px;overflow:auto;">
      {#each selectedConns as c}
        <li>{c.name}{c.group_name ? ` · ${c.group_name}` : ''}</li>
      {/each}
    </ul>
    {#if needsAck}
      <label style="display:flex;gap:6px;align-items:center;font-size:13px;margin-bottom:10px;">
        <input type="checkbox" bind:checked={acknowledged} />
        I understand this runs on all {selectedConns.length} servers.
      </label>
    {/if}
    <div class="row" style="justify-content:flex-end;gap:8px;">
      <button class="btn-secondary" on:click={() => (confirming = false)}>Cancel</button>
      <button class="btn-primary" disabled={needsAck && !acknowledged} on:click={() => void run()}>
        <Icon name="play" /> Apply
      </button>
    </div>
  </Modal>
{/if}
