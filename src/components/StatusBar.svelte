<script lang="ts">
  import { notification } from '../stores/notification';
  import { fly } from 'svelte/transition';

  $: visible = $notification.kind !== 'idle';
</script>

{#if visible}
  <div
    class="toast toast-{$notification.kind}"
    role={$notification.kind === 'error' ? 'alert' : 'status'}
    aria-live="polite"
    transition:fly={{ y: 16, duration: 200 }}
  >
    {#if $notification.kind === 'busy'}
      <span class="toast-spinner" aria-hidden="true"></span>
    {:else if $notification.kind === 'ok'}
      <span class="toast-icon" aria-hidden="true">✓</span>
    {:else if $notification.kind === 'error'}
      <span class="toast-icon" aria-hidden="true">✕</span>
    {/if}
    <span class="toast-msg">{$notification.msg}</span>
  </div>
{/if}

<style>
  .toast {
    position: fixed;
    bottom: 20px;
    right: 20px;
    z-index: 1000;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 16px;
    border-radius: 8px;
    font-size: 13px;
    font-weight: 500;
    max-width: 420px;
    min-width: 200px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.35);
    pointer-events: none;
    white-space: nowrap;
    overflow: hidden;
  }

  .toast-busy {
    background: var(--chip-bg);
    color: var(--text-accent);
    border: 1px solid var(--btn-primary-bg);
  }

  .toast-ok {
    background: #14532d;
    color: #86efac;
    border: 1px solid var(--text-ok);
  }

  .toast-error {
    background: var(--error-bg);
    color: var(--text-danger);
    border: 1px solid var(--error-color);
  }

  :global([data-theme="light"]) .toast-ok {
    background: #dcfce7;
    color: #15803d;
    border-color: var(--text-ok);
  }

  .toast-msg {
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
  }

  .toast-icon {
    flex-shrink: 0;
    font-size: 12px;
    font-weight: 700;
  }

  .toast-spinner {
    flex-shrink: 0;
    width: 13px;
    height: 13px;
    border: 2px solid currentColor;
    border-top-color: transparent;
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    opacity: 0.85;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }
</style>
