<script lang="ts">
  import { notification } from '../stores/notification';
  import { fly } from 'svelte/transition';
  import { openFolder, openFile } from '../api';

  $: visible = $notification.kind === 'ok' || $notification.kind === 'error';

  function onOpenFolder() {
    if ($notification.openFolder) openFolder($notification.openFolder);
  }
  function onOpenFile() {
    if ($notification.openFile) openFile($notification.openFile);
  }
</script>

{#if visible}
  <div
    class="toast toast-{$notification.kind}"
    role={$notification.kind === 'error' ? 'alert' : 'status'}
    aria-live="polite"
    transition:fly={{ y: 16, duration: 200 }}
  >
    <span class="toast-icon" aria-hidden="true">{$notification.kind === 'ok' ? '✓' : '✕'}</span>
    <span class="toast-msg">{$notification.msg}</span>
    {#if $notification.openFolder}
      <button class="toast-action-btn" on:click={onOpenFolder} title="Open folder">📂</button>
    {/if}
    {#if $notification.openFile}
      <button class="toast-action-btn" on:click={onOpenFile} title="Open file">📄</button>
    {/if}
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
    pointer-events: all;
    white-space: nowrap;
    overflow: hidden;
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

  .toast-action-btn {
    flex-shrink: 0;
    background: none;
    border: none;
    cursor: pointer;
    font-size: 15px;
    padding: 0 2px;
    line-height: 1;
    opacity: 0.85;
  }
  .toast-action-btn:hover { opacity: 1; }
</style>
