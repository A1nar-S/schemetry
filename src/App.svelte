<script lang="ts">
  import Icon from './components/Icon.svelte';
  import { onMount } from 'svelte';
  import { getVersion } from '@tauri-apps/api/app';
  import { getConnections, mcpAuditVerify } from './api';
  import type { ConnectionRecord } from './types';
  import { notify } from './stores/notification';
  import { auditVerification } from './stores/mcpAuditViewState';
  import BusyOverlay from './components/BusyOverlay.svelte';
  import Toast from './components/Toast.svelte';
  import TitleBar from './components/TitleBar.svelte';
  import QueryView from './views/QueryView.svelte';
  import FixView from './views/FixView.svelte';
  import DdlView from './views/DdlView.svelte';
  import ApplyView from './views/ApplyView.svelte';
  import HistoryFixView from './views/HistoryFixView.svelte';
  import McpAuditView from './views/McpAuditView.svelte';
  import ConnectionsView from './views/ConnectionsView.svelte';
  import SettingsView from './views/SettingsView.svelte';

  type View = 'query' | 'apply' | 'compare' | 'ddl' | 'historyfix' | 'mcpaudit' | 'connections' | 'settings';

  const VIEW_TITLES: Record<View, string> = {
    query:       'Query',
    apply:       'Apply DDL',
    compare:     'Fix Discrepancies',
    ddl:         'Generate DDL',
    historyfix:  'Fix History Tables',
    mcpaudit:    'MCP Audit',
    connections: 'Connections',
    settings:    'Settings',
  };

  let activeView: View = 'query';
  let connections: ConnectionRecord[] = [];

  // The integrity banner can be dismissed for this session; a different problem shows it again.
  let dismissedProblem: string | null = null;
  $: problemKey = $auditVerification && !$auditVerification.ok
    ? `${$auditVerification.problem ?? ''}|${$auditVerification.first_bad_id ?? ''}`
    : null;
  $: showAuditBanner = problemKey !== null && problemKey !== dismissedProblem;
  const COLLAPSE_BREAKPOINT = 900;
  const SIDEBAR_KEY = 'schemetry-sidebar-collapsed';

  // The user's explicit choice is persisted and never overridden by resizing.
  // Shrinking below the breakpoint auto-collapses an expanded sidebar; growing
  // back re-expands it only if that collapse was automatic.
  let sidebarCollapsed = localStorage.getItem(SIDEBAR_KEY) === 'true';
  let autoCollapsed = false;
  let wasNarrow = false;
  let appVersion = '';

  function handleResize() {
    const narrow = window.innerWidth < COLLAPSE_BREAKPOINT;
    if (narrow && !wasNarrow && !sidebarCollapsed) {
      sidebarCollapsed = true;
      autoCollapsed = true;
    } else if (!narrow && wasNarrow && autoCollapsed) {
      sidebarCollapsed = false;
      autoCollapsed = false;
    }
    wasNarrow = narrow;
  }

  onMount(() => {
    handleResize();
    window.addEventListener('resize', handleResize);
    loadConnections();
    getVersion().then(v => (appVersion = v)).catch(() => {});
    // Fire-and-forget: a failing check must never hold up or break startup.
    mcpAuditVerify().then(r => auditVerification.set(r)).catch(() => {});
    return () => window.removeEventListener('resize', handleResize);
  });

  function toggleSidebar() {
    sidebarCollapsed = !sidebarCollapsed;
    autoCollapsed = false;
    localStorage.setItem(SIDEBAR_KEY, String(sidebarCollapsed));
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
<TitleBar title={VIEW_TITLES[activeView]} {sidebarCollapsed} onToggleSidebar={toggleSidebar} />
{#if showAuditBanner}
  <div class="mcp-notice mcp-notice-danger audit-banner" role="alert">
    <Icon name="alert-triangle" size={15} />
    <span class="audit-banner-text">
      <strong>MCP audit log integrity problem:</strong> {$auditVerification?.problem ?? 'the hash chain does not verify.'}
    </span>
    <button class="btn-secondary" style="font-size:12px;padding:2px 10px;" on:click={() => (activeView = 'mcpaudit')}>Open MCP audit</button>
    <button class="btn-secondary" style="font-size:12px;padding:2px 8px;" title="Dismiss for this session" aria-label="Dismiss" on:click={() => (dismissedProblem = problemKey)}>
      <Icon name="x" size={12} />
    </button>
  </div>
{/if}
<div class="layout" class:sidebar-collapsed={sidebarCollapsed}>
  <!-- Sidebar -->
  <aside class="sidebar">
    <nav class="nav-links">
      <button
        class="nav-btn"
        class:active={activeView === 'query'}
        title="Query"
        on:click={() => (activeView = 'query')}
      >
        <Icon name="search" size={16} /><span class="nav-label">Query</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'apply'}
        title="Apply DDL"
        on:click={() => (activeView = 'apply')}
      >
        <Icon name="play" size={16} /><span class="nav-label">Apply DDL</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'ddl'}
        title="Generate DDL"
        on:click={() => (activeView = 'ddl')}
      >
        <Icon name="file-code" size={16} /><span class="nav-label">Generate DDL</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'compare'}
        title="Fix Discrepancies"
        on:click={() => (activeView = 'compare')}
      >
        <Icon name="git-compare" size={16} /><span class="nav-label">Fix Discrepancies</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'historyfix'}
        title="Fix History Tables"
        on:click={() => (activeView = 'historyfix')}
      >
        <Icon name="history" size={16} /><span class="nav-label">Fix History Tables</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'mcpaudit'}
        title="MCP Audit"
        on:click={() => (activeView = 'mcpaudit')}
      >
        <Icon name="shield-check" size={16} /><span class="nav-label">MCP Audit</span>
      </button>

      <div class="nav-divider"></div>
      <div class="nav-separator"></div>

      <button
        class="nav-btn"
        class:active={activeView === 'connections'}
        title="Connections"
        on:click={() => (activeView = 'connections')}
      >
        <Icon name="database" size={16} /><span class="nav-label">Connections</span>
      </button>
      <button
        class="nav-btn"
        class:active={activeView === 'settings'}
        title="Settings"
        on:click={() => (activeView = 'settings')}
      >
        <Icon name="settings" size={16} /><span class="nav-label">Settings</span>
      </button>
    </nav>
  </aside>

  <!-- Content -->
  <div class="content-area">
    <div class="view-body">
      {#if activeView === 'query'}
        <QueryView {connections} />
      {:else if activeView === 'apply'}
        <ApplyView {connections} />
      {:else if activeView === 'compare'}
        <FixView {connections} />
      {:else if activeView === 'ddl'}
        <DdlView {connections} />
      {:else if activeView === 'historyfix'}
        <HistoryFixView {connections} />
      {:else if activeView === 'mcpaudit'}
        <McpAuditView {connections} />
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
    {#if appVersion}<span class="status-version">{appVersion === '0.0.0' ? 'dev' : `v${appVersion}`}</span>{/if}
  </footer>
</div>

<BusyOverlay />
<Toast />

