<script lang="ts">
  import { busy, notification } from '../stores/notification';
  import { cancelTask } from '../api';
  import Modal from './Modal.svelte';

  let stopping = false;
  $: if (!$busy) stopping = false;

  // The busy action then fails with "Cancelled by user." and clears the overlay itself.
  function onStop() {
    stopping = true;
    cancelTask().catch(() => {});
  }
</script>

{#if $busy}
  <Modal width="360px">
    <div class="modal-header">
      <span class="modal-title">Working</span>
    </div>
    <div class="modal-body-pad">
      <p class="muted">{stopping ? 'Stopping…' : $notification.msg}</p>
      <div class="progress-track"></div>
    </div>
    <div class="modal-footer">
      <button class="btn-secondary" on:click={onStop} disabled={stopping}>Stop</button>
    </div>
  </Modal>
{/if}

<style>
  .modal-body-pad {
    padding: 8px 4px 4px;
  }
  .muted {
    color: var(--text-muted);
    margin: 0 0 12px;
    font-size: 13px;
  }
  /* Background-image bar, not a clipped child: avoids WebView2 repaint trails. */
  .progress-track {
    width: 100%;
    height: 6px;
    border-radius: 999px;
    background-color: var(--border-subtle);
    background-image: linear-gradient(90deg, var(--text-link), var(--text-accent));
    background-size: 38% 100%;
    background-repeat: no-repeat;
    animation: progress-slide 1.1s ease-in-out infinite;
  }
  @keyframes progress-slide {
    0%   { background-position: -62% 0; }
    100% { background-position: 162% 0; }
  }
</style>
