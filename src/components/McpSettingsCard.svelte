<script lang="ts">
  import { onMount } from 'svelte';
  import type { McpAuditSinkKind, McpSettings } from '../types';
  import { getMcpSettings, saveMcpSettings, testMcpAuditSink } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';

  const SINKS: { value: McpAuditSinkKind; label: string }[] = [
    { value: '', label: 'None' },
    { value: 'syslog', label: "Syslog (UDP host:port, or 'local' on Linux/macOS)" },
    { value: 'eventlog', label: 'Windows Event Log' },
    { value: 'http', label: 'HTTP endpoint (POST JSON)' },
  ];
  const PLACEHOLDERS: Record<McpAuditSinkKind, string> = {
    '': '',
    syslog: "e.g. 10.0.0.5:514, or 'local'",
    eventlog: '',
    http: 'e.g. https://siem.example.com/ingest',
  };

  let dataToolsDisabled = false;
  let sinkKind: McpAuditSinkKind = '';
  let sinkTarget = '';
  let baseline = '';

  $: needsTarget = sinkKind === 'syslog' || sinkKind === 'http';
  $: current = snapshot(dataToolsDisabled, sinkKind, sinkTarget);
  $: dirty = baseline !== '' && current !== baseline;

  function snapshot(disabled: boolean, kind: McpAuditSinkKind, target: string): string {
    return JSON.stringify([disabled, kind, needsTargetFor(kind) ? target.trim() : '']);
  }

  function needsTargetFor(kind: McpAuditSinkKind): boolean {
    return kind === 'syslog' || kind === 'http';
  }

  function apply(s: McpSettings) {
    dataToolsDisabled = s.data_tools_disabled;
    sinkKind = s.audit_sink_kind;
    sinkTarget = s.audit_sink_target;
    baseline = snapshot(dataToolsDisabled, sinkKind, sinkTarget);
  }

  onMount(async () => {
    try {
      apply(await getMcpSettings());
    } catch (e) {
      notify(`Failed to load MCP settings: ${String(e)}`, 'error');
    }
  });

  function onKindChange() {
    if (!needsTarget) sinkTarget = '';
  }

  // Persists only when something changed. Throws on failure so callers can report it.
  export async function saveIfDirty(): Promise<void> {
    if (!dirty) return;
    if (needsTarget && !sinkTarget.trim()) {
      throw new Error('MCP audit sink target is required.');
    }
    await saveMcpSettings({
      data_tools_disabled: dataToolsDisabled,
      audit_sink_kind: sinkKind,
      audit_sink_target: needsTarget ? sinkTarget.trim() : '',
    });
    baseline = snapshot(dataToolsDisabled, sinkKind, sinkTarget);
  }

  async function onSave() {
    setBusy(true, 'Saving MCP settings…');
    try {
      await saveIfDirty();
      notify('MCP settings saved.', 'ok');
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onTest() {
    if (needsTarget && !sinkTarget.trim()) {
      notify('Enter the sink target first.', 'error');
      return;
    }
    setBusy(true, 'Sending test message…');
    try {
      await testMcpAuditSink(sinkKind, needsTarget ? sinkTarget.trim() : '');
      notify('Test message sent to the audit sink.', 'ok');
    } catch (e) {
      notify(`Audit sink test failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="card stack">
  <div class="section-title">MCP server</div>
  <p class="hint">
    Lets external AI agents use your connections through the separate <code>schemetry-mcp</code> process, under the per-connection
    policies set in the Connections view. Every agent call is recorded in the MCP audit log.
  </p>

  <label class="check-row">
    <input type="checkbox" bind:checked={dataToolsDisabled} />
    <span>Disable all MCP data tools (<code>query_table</code>, <code>run_query</code>) regardless of per-connection policy</span>
  </label>

  <div class="field-row">
    <div class="field">
      <span>External audit sink</span>
      <select bind:value={sinkKind} on:change={onKindChange}>
        {#each SINKS as s}
          <option value={s.value}>{s.label}</option>
        {/each}
      </select>
    </div>
    {#if needsTarget}
      <div class="field" style="flex:2;">
        <span>Target</span>
        <input bind:value={sinkTarget} placeholder={PLACEHOLDERS[sinkKind]} autocomplete="off" />
      </div>
    {/if}
  </div>

  <div class="row">
    <button class="btn-secondary" on:click={() => void onTest()} disabled={$busy || sinkKind === ''}>
      Send test message
    </button>
    <button class="btn-primary" on:click={() => void onSave()} disabled={$busy || !dirty}>
      Save MCP settings
    </button>
    {#if dirty}<span class="hint">Unsaved changes</span>{/if}
  </div>

  <p class="hint">
    On a machine you own, no local file is truly immutable. Schemetry has no way to delete or edit the audit log, and any
    tampering is detected by the hash chain — but a truly immutable copy requires an external sink.
  </p>
</div>

<style>
  .check-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-primary);
  }
  .field-row {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .field-row .field {
    flex: 1;
    min-width: 200px;
  }
  .field select {
    font-size: 13px;
    padding: 6px 10px;
    width: 100%;
  }
</style>
