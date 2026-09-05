<script lang="ts">
  import { onMount } from 'svelte';
  import { getVersion } from '@tauri-apps/api/app';
  import { getConnections } from './api';
  import type { ConnectionRecord } from './types';
  import { theme } from './hooks/useTheme';
  import { notify } from './stores/notification';
  import BusyOverlay from './components/BusyOverlay.svelte';
  import Toast from './components/Toast.svelte';
  import QueryView from './views/QueryView.svelte';
  import FixView from './views/FixView.svelte';
  import DdlView from './views/DdlView.svelte';
  import HistoryFixView from './views/HistoryFixView.svelte';
  import ConnectionsView from './views/ConnectionsView.svelte';
  import SettingsView from './views/SettingsView.svelte';

  type View = 'query' | 'compare' | 'ddl' | 'historyfix' | 'connections' | 'settings';

  const VIEW_TITLES: Record<View, string> = {
    query:       'Query',
    compare:     'Fix Discrepancies',
    ddl:         'Generate DDL',
    historyfix:  'Fix History Tables',
    connections: 'Connections',
    settings:    'Settings',
  };

  let activeView: View = 'query';
  let connections: ConnectionRecord[] = [];
  let sidebarCollapsed = false;
  let userToggled = false;
  let appVersion = '';

  const COLLAPSE_BREAKPOINT = 900;

  function handleResize() {
    const shouldCollapse = window.innerWidth < COLLAPSE_BREAKPOINT;
    if (shouldCollapse !== sidebarCollapsed) {
      userToggled = false;
    }
    if (!userToggled) {
      sidebarCollapsed = shouldCollapse;
    }
  }

  onMount(() => {
    handleResize();
    window.addEventListener('resize', handleResize);
    loadConnections();
    getVersion().then(v => (appVersion = v)).catch(() => {});
    return () => window.removeEventListener('resize', handleResize);
  });

  function toggleSidebar() {
    userToggled = true;
    sidebarCollapsed = !sidebarCollapsed;
  }

  async function loadConnections() {
    try {
      connections = await getConnections();
    } catch (e) {
      notify(`Failed to load connections: ${String(e)}`, 'error');
    }
  }
</script>

<div class="app-shell">
<div class="layout" class:sidebar-collapsed={sidebarCollapsed}>
  <!-- Sidebar -->
  <aside class="sidebar">
    <div class="brand">
      {#if !sidebarCollapsed}Schemetry{/if}
      <button class="collapse-btn" on:click={toggleSidebar} title={sidebarCollapsed ? 'Expand sidebar' : 'Collapse sidebar'}>
        {sidebarCollapsed ? '›' : '‹'}
      </button>
    </div>
    <nav class="nav-links">
      <button
        class="nav-btn"
        class:active={activeView === 'query'}
        title="Query"
        on:click={() => (activeView = 'query')}
      >
        <span>🔍</span><span class="nav-label"> Query</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'ddl'}
        title="Generate DDL"
        on:click={() => (activeView = 'ddl')}
      >
        <span>📄</span><span class="nav-label"> Generate DDL</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'compare'}
        title="Fix Discrepancies"
        on:click={() => (activeView = 'compare')}
      >
        <span>🛠️</span><span class="nav-label"> Fix Discrepancies</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'historyfix'}
        title="Fix History Tables"
        on:click={() => (activeView = 'historyfix')}
      >
        <span>🗃️</span><span class="nav-label"> Fix History Tables</span>
      </button>

      <div class="nav-divider"></div>
      <div class="nav-separator"></div>

      <button
        class="nav-btn"
        class:active={activeView === 'connections'}
        title="Connections"
        on:click={() => (activeView = 'connections')}
      >
        <span>🔗</span><span class="nav-label"> Connections</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'settings'}
        title="Settings"
        on:click={() => (activeView = 'settings')}
      >
        <span>⚙️</span><span class="nav-label"> Settings</span>
      </button>
    </nav>
    <button
      class="theme-toggle"
      on:click={theme.toggle}
      title={$theme === 'dark' ? 'Dark theme (click for light)' : $theme === 'light' ? 'Light theme (click for auto)' : 'Auto theme (click for dark)'}
    >
      {#if sidebarCollapsed}
        {$theme === 'dark' ? '🌙' : $theme === 'light' ? '☀' : '🌓'}
      {:else}
        {$theme === 'dark' ? '🌙 Dark mode' : $theme === 'light' ? '☀ Light mode' : '🌓 Auto'}
      {/if}
    </button>
  </aside>

  <!-- Content -->
  <div class="content-area">
    <div class="view-header">
      <span class="view-title">{VIEW_TITLES[activeView]}</span>
    </div>
    <div class="view-body">
      {#if activeView === 'query'}
        <QueryView {connections} />
      {:else if activeView === 'compare'}
        <FixView {connections} />
      {:else if activeView === 'ddl'}
        <DdlView {connections} />
      {:else if activeView === 'historyfix'}
        <HistoryFixView {connections} />
      {:else if activeView === 'connections'}
        <ConnectionsView {connections} onReload={loadConnections} />
      {:else}
        <SettingsView {connections} />
      {/if}
    </div>
  </div>
</div>

  <!-- Bottom status bar -->
  <footer class="status-bar">
    {#if appVersion}<span class="status-version">v{appVersion}</span>{/if}
  </footer>
</div>

<BusyOverlay />
<Toast />

