<script lang="ts">
  import { busy } from '../stores/notification';
  import type { LoadedServer } from '../types';

  export let loadedServers: LoadedServer[] = [];
  export let referenceServer: number | null = null;
  export let checkComments = false;
  export let checkIndexes = false;
  export let onReferenceChange: (id: number | null) => void;
  export let onToggleComments: (v: boolean) => void;
  export let onToggleIndexes: (v: boolean) => void;
  export let onRunComparison: () => void;

  // <select> values are always strings; translate to/from the numeric id.
  $: selectValue = referenceServer === null ? '' : String(referenceServer);
  function onSelectChange(raw: string) {
    onReferenceChange(raw === '' ? null : Number(raw));
  }
</script>

<div class="card stack">
  <div class="section-title">Comparison Options</div>

  <div class="field">
    <span>Reference server</span>
    <select
      value={selectValue}
      on:change={(e) => onSelectChange(e.currentTarget.value)}
      disabled={!loadedServers.length}
    >
      <option value="">— select —</option>
      {#each loadedServers as s (s.id)}
        <option value={s.id}>{s.name}</option>
      {/each}
    </select>
  </div>

  <label class="check-row">
    <input type="checkbox" checked={checkComments} on:change={() => onToggleComments(!checkComments)} />
    Compare comments
  </label>
  <label class="check-row">
    <input type="checkbox" checked={checkIndexes} on:change={() => onToggleIndexes(!checkIndexes)} />
    Compare indexes
  </label>

  <button
    class="btn-primary"
    on:click={onRunComparison}
    disabled={$busy || !referenceServer}
  >🔍 Run Comparison</button>
</div>

<style>
  .check-row {
    display: flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }
</style>
