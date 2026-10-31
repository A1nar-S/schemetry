<script lang="ts">
  import Icon from './Icon.svelte';
  import { onMount } from 'svelte';
  import { getCurrentWindow } from '@tauri-apps/api/window';

  export let title: string;
  export let sidebarCollapsed: boolean;
  export let onToggleSidebar: () => void;

  // Window controls only work inside Tauri; `npm run dev` in a plain browser hides them.
  const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
  const appWindow = inTauri ? getCurrentWindow() : null;
  let maximized = false;

  async function syncMaximized() {
    maximized = (await appWindow?.isMaximized().catch(() => false)) ?? false;
  }

  onMount(() => {
    if (!appWindow) return;
    void syncMaximized();
    const unlisten = appWindow.onResized(() => void syncMaximized());
    return () => void unlisten.then(fn => fn());
  });
</script>

<!--
  The whole bar is a drag region. Tauri only honours `data-tauri-drag-region`
  on the element itself, so the text inside carries it too. Double-clicking a
  drag region toggles maximize.
-->
<header class="titlebar" class:sidebar-collapsed={sidebarCollapsed} data-tauri-drag-region>
  <div class="titlebar-brand" data-tauri-drag-region>
    {#if !sidebarCollapsed}<span class="titlebar-brand-name" data-tauri-drag-region>Schemetry</span>{/if}
    <button class="collapse-btn" on:click={onToggleSidebar} title={sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
      <Icon name={sidebarCollapsed ? 'chevron-right' : 'chevron-left'} size={16} />
    </button>
  </div>
  <div class="titlebar-title" data-tauri-drag-region>
    <span data-tauri-drag-region>{title}</span>
  </div>
  {#if appWindow}
    <div class="window-controls">
      <button class="window-btn" title="Minimize" aria-label="Minimize" on:click={() => void appWindow.minimize()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5.5h10" /></svg>
      </button>
      <button
        class="window-btn"
        title={maximized ? 'Restore' : 'Maximize'}
        aria-label={maximized ? 'Restore' : 'Maximize'}
        on:click={() => void appWindow.toggleMaximize()}
      >
        {#if maximized}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <path d="M2.5 2.5V.5h7v7h-2" /><rect x=".5" y="2.5" width="7" height="7" />
          </svg>
        {:else}
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><rect x=".5" y=".5" width="9" height="9" /></svg>
        {/if}
      </button>
      <button class="window-btn window-btn-close" title="Close" aria-label="Close" on:click={() => void appWindow.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M.5.5l9 9M9.5.5l-9 9" /></svg>
      </button>
    </div>
  {/if}
</header>
