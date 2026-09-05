<script lang="ts">
  import { busy } from '../stores/notification';

  export let loadedServers: string[] = [];
  export let onOpenSelector: () => void;
</script>

<div class="card stack">
  <div class="section-title">Data Sources</div>

  <div class="chips-wrap">
    {#if loadedServers.length === 0}
      <span class="empty-state">No servers loaded yet.</span>
    {:else}
      {#each loadedServers as s}
        <span class="chip">{s}</span>
      {/each}
    {/if}
  </div>

  <div class="row">
    <button class="btn-primary" on:click={onOpenSelector} disabled={$busy}>
      🔌Select and Fetch Servers
    </button>
  </div>
</div>
