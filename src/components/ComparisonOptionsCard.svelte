<script lang="ts">
  import { busy } from '../stores/notification';
  import type { LoadedServer } from '../types';

  export let loadedServers: LoadedServer[] = [];
  export let referenceServer: number | null = null;
  export let checkTables = true;
  export let checkColumns = true;
  export let checkComments = false;
  export let checkIndexes = false;
  export let onReferenceChange: (id: number | null) => void;
  export let onToggleTables: (v: boolean) => void;
  export let onToggleColumns: (v: boolean) => void;
  export let onToggleComments: (v: boolean) => void;
  export let onToggleIndexes: (v: boolean) => void;
  export let onRunComparison: () => void;

  // Pass referenceServer through untouched (no String() coercion): Svelte's
  // <select value={...}> matches option values by identity (Object.is) against
  // each <option>'s raw JS value, so a stringified copy never matches the
  // numeric ids below and forces the select blank on every change.
  function onSelectChange(raw: string) {
    onReferenceChange(raw === '' ? null : Number(raw));
  }

  $: nothingSelected = !checkTables && !checkColumns && !checkComments && !checkIndexes;
</script>

<div class="card stack">
  <div class="section-title">Comparison Options</div>

  <div class="field">
    <span>Reference server</span>
    <select
      value={referenceServer}
      on:change={(e) => onSelectChange(e.currentTarget.value)}
      disabled={!loadedServers.length}
    >
      <option value={null}>— select —</option>
      {#each loadedServers as s (s.id)}
        <option value={s.id}>{s.name}</option>
      {/each}
    </select>
  </div>

  <div class="field">
    <span>Compare</span>
    <div class="check-grid">
      <label class="check-row">
        <input type="checkbox" checked={checkTables} on:change={() => onToggleTables(!checkTables)} />
        Tables
      </label>
      <label class="check-row">
        <input type="checkbox" checked={checkColumns} on:change={() => onToggleColumns(!checkColumns)} />
        Columns
      </label>
      <label class="check-row">
        <input type="checkbox" checked={checkIndexes} on:change={() => onToggleIndexes(!checkIndexes)} />
        Indexes
      </label>
      <label class="check-row">
        <input type="checkbox" checked={checkComments} on:change={() => onToggleComments(!checkComments)} />
        Comments
      </label>
    </div>
    {#if nothingSelected}
      <span class="hint" style="margin:0;">Select at least one item to compare.</span>
    {/if}
  </div>

  <button
    class="btn-primary"
    on:click={onRunComparison}
    disabled={$busy || !referenceServer || nothingSelected}
  >🔍 Run Comparison</button>
</div>

<style>
  .check-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 4px 12px;
  }
  .check-row {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
</style>
