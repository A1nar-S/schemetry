<script lang="ts">
  import { busy } from '../stores/notification';
  import type { LoadedServer } from '../types';

  export let loadedServers: LoadedServer[] = [];
  export let onOpenSelector: () => void;
</script>

<div class="card stack">
  <div class="section-title">Data Sources</div>

  <div class="chips-wrap">
    {#if loadedServers.length === 0}
      <span class="empty-state">No servers loaded yet.</span>
    {:else}
      {#each loadedServers as s (s.id)}
        <span class="chip">{s.name}</span>
      {/each}
    {/if}
  </div>

  <div class="row">
    <button class="btn-primary" on:click={onOpenSelector} disabled={$busy}>
      🔌Select and Fetch Servers
    </button>
  </div>
</div>
