<script lang="ts">
  import { busy } from '../stores/notification';

  export let loadedServers: string[] = [];
  export let referenceServer = '';
  export let checkComments = false;
  export let checkIndexes = false;
  export let onReferenceChange: (s: string) => void;
  export let onToggleComments: (v: boolean) => void;
  export let onToggleIndexes: (v: boolean) => void;
  export let onRunComparison: () => void;
</script>

<div class="card stack">
  <div class="section-title">Comparison Options</div>

  <div class="field">
    <span>Reference server</span>
    <select
      bind:value={referenceServer}
      on:change={() => onReferenceChange(referenceServer)}
      disabled={!loadedServers.length}
    >
      <option value="">— select —</option>
      {#each loadedServers as s}
        <option value={s}>{s}</option>
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
