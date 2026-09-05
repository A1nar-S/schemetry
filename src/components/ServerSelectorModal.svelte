<script lang="ts">
  import type { ConnectionRecord } from '../types';
  import { busy } from '../stores/notification';
  import Modal from './Modal.svelte';

  export let connections: ConnectionRecord[];
  export let selectedForFetch: Set<string>;
  export let onClose: () => void;
  export let onFetch: () => void;
  export let actionLabel: string = '📥 Fetch Metadata';
  export let onToggle: (name: string) => void;
  export let onSelectAll: () => void;
  export let onSelectNone: () => void;
  export let onSelectSchema: (schema: string) => void;
  export let onDeselectSchema: (schema: string) => void;

  $: schemaGroups = buildGroups(connections);

  function buildGroups(conns: ConnectionRecord[]): Map<string, ConnectionRecord[]> {
    const g = new Map<string, ConnectionRecord[]>();
    for (const c of conns) {
      const key = c.group_name?.trim() || 'Default';
      if (!g.has(key)) g.set(key, []);
      g.get(key)!.push(c);
    }
    return g;
  }
</script>

<Modal width="520px" {onClose}>
  <div class="modal-header">
    <span class="modal-title">Select Servers to Fetch</span>
    <button class="btn-secondary" style="padding:2px 8px;" on:click={onClose}>✕</button>
  </div>

  <div class="row" style="margin-bottom:8px;">
    <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={onSelectAll}>Select All</button>
    <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={onSelectNone}>Select None</button>
  </div>

  <div class="server-list">
    {#each [...schemaGroups.entries()] as [schema, conns]}
      <div class="schema-group">
        <div class="schema-group-header">
          <span class="schema-group-name">{schema}</span>
          <span class="schema-group-count">({conns.length})</span>
          <button class="btn-xs" on:click={() => onSelectSchema(schema)}>All</button>
          <button class="btn-xs" on:click={() => onDeselectSchema(schema)}>None</button>
        </div>
        <div class="schema-group-items">
          {#each conns as conn}
            <label class="schema-item">
              <input
                type="checkbox"
                checked={selectedForFetch.has(conn.name)}
                on:change={() => onToggle(conn.name)}
              />
              <span style="margin-left:4px;">{conn.name}</span>
              <span style="margin-left:auto;font-size:11px;color:#475569;">{conn.host}</span>
            </label>
          {/each}
        </div>
      </div>
    {:else}
      <div class="empty-state">No connections configured.</div>
    {/each}
  </div>

  <div class="modal-footer">
    <button
      class="btn-primary"
      on:click={onFetch}
      disabled={$busy || !selectedForFetch.size}
    >{actionLabel} ({selectedForFetch.size} selected)</button>
  </div>
</Modal>

<style>
  .server-list {
    max-height: 340px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .modal-footer {
    display: flex;
    justify-content: flex-end;
    margin-top: 14px;
  }
</style>
