<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import type { McpApplyRequest } from '../types';
  import { decideMcpApplyRequest, listMcpApplyRequests } from '../api';
  import { notify } from '../stores/notification';

  /** Called after a decision, so the audit list can be refreshed. */
  export let onDecided: () => void = () => {};

  const POLL_MS = 5000;

  let requests: McpApplyRequest[] = [];
  let timer: ReturnType<typeof setInterval> | undefined;
  let deciding: number | null = null;

  async function load() {
    try {
      requests = await listMcpApplyRequests();
    } catch (e) {
      notify(`Failed to load pending deploys: ${String(e)}`, 'error');
    }
  }

  async function decide(request: McpApplyRequest, approve: boolean) {
    if (approve && !confirm(
      `Approve deploying ${request.summary} ${request.object_name} to '${request.connection_name}'?\n\n` +
      'The agent can then run exactly this script once within 15 minutes.',
    )) return;
    deciding = request.id;
    try {
      await decideMcpApplyRequest(request.id, approve);
      notify(approve ? 'Deploy approved. The agent can now retry.' : 'Deploy rejected.', 'ok');
      onDecided();
    } catch (e) {
      notify(String(e), 'error');
    } finally {
      deciding = null;
      await load();
    }
  }

  function expires(unix: number): string {
    return new Date(unix * 1000).toLocaleTimeString();
  }

  onMount(() => {
    void load();
    timer = setInterval(() => void load(), POLL_MS);
  });
  onDestroy(() => {
    if (timer) clearInterval(timer);
  });
</script>

{#if requests.length > 0}
  <div class="card stack">
    <div class="section-title">Pending deploys ({requests.length})</div>
    <p class="hint">
      Agents asked to deploy these scripts through <code>apply_ddl</code>. Review the SQL before approving: an approval covers exactly this script, once.
    </p>
    {#each requests as r (r.id)}
      <div class="request">
        <div class="row">
          <strong>{r.summary} {r.object_name}</strong>
          <span class="hint">on {r.connection_name || `connection ${r.connection_id}`}</span>
          <div class="spacer"></div>
          {#if r.status === 'pending'}
            <button class="btn-primary" disabled={deciding === r.id} on:click={() => void decide(r, true)}>
              <Icon name="check" /> Approve
            </button>
            <button class="btn-danger" disabled={deciding === r.id} on:click={() => void decide(r, false)}>
              <Icon name="x" /> Reject
            </button>
          {:else}
            <span class="hint">Approved — waiting for the agent to retry (until {expires(r.expires_at)})</span>
          {/if}
        </div>
        <div class="hint mono">
          {r.bytes} bytes · sha256 {r.sha256}{r.status === 'pending' ? ` · open until ${expires(r.expires_at)}` : ''}
        </div>
        <details open={r.status === 'pending'}>
          <summary>SQL</summary>
          <pre class="ddl">{r.ddl}</pre>
        </details>
      </div>
    {/each}
  </div>
{/if}

<style>
  .request {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px 0;
    border-top: 1px solid var(--border, rgba(128, 128, 128, 0.3));
  }
  .mono {
    font-family: ui-monospace, Consolas, monospace;
    word-break: break-all;
  }
  .ddl {
    max-height: 260px;
    overflow: auto;
    margin: 4px 0 0;
    padding: 8px;
    font-size: 12px;
    white-space: pre;
    background: var(--bg-input, rgba(128, 128, 128, 0.1));
    border-radius: 4px;
  }
</style>
