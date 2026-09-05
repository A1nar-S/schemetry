<script lang="ts">
  import { busy, notification } from '../stores/notification';
  import Modal from './Modal.svelte';
</script>

{#if $busy}
  <Modal width="360px">
    <div class="modal-header">
      <span class="modal-title">Working</span>
    </div>
    <div class="modal-body-pad">
      <p class="muted">{$notification.msg}</p>
      <div class="progress-track">
        <div class="progress-indeterminate"></div>
      </div>
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
  .progress-track {
    width: 100%;
    height: 6px;
    border-radius: 999px;
    background: var(--border-subtle);
    overflow: hidden;
  }
  .progress-indeterminate {
    height: 100%;
    width: 38%;
    border-radius: 999px;
    background: linear-gradient(90deg, var(--text-link), var(--text-accent));
    animation: progress-slide 1.1s ease-in-out infinite;
  }
  @keyframes progress-slide {
    0%   { transform: translateX(-110%); }
    100% { transform: translateX(360%); }
  }
</style>
