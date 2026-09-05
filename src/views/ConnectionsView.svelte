<script lang="ts">
  import type { ConnectionRecord } from '../types';
  import { deleteAllConnections, deleteConnection, exportConnections, importConnections, openInPlsqlDeveloper, saveConnection, testConnection } from '../api';
  import { save } from '@tauri-apps/plugin-dialog';
  import { busy, setBusy, notify } from '../stores/notification';
  import Modal from '../components/Modal.svelte';

  export let connections: ConnectionRecord[];
  export let onReload: () => Promise<void>;

  const emptyForm = (): ConnectionRecord => ({
    id: 0,
    name: '',
    host: '',
    port: 1521,
    service_name: '',
    username: '',
    password: '',
    group_name: '',
  });

  let form: ConnectionRecord = emptyForm();
  let editingId: number | undefined;

  // JSON import
  let showImport = false;
  let showPassword = false;
  let importJson = '';
  let importError = '';

  // Delete-all confirmation
  let showDeleteAllModal = false;

  // ── Quick search + schema groups ───────────────────────────────────
  let searchQuery = '';

  $: filteredConnections = searchQuery.trim()
    ? connections.filter(c => {
        const q = searchQuery.trim().toLowerCase();
        return (
          c.name.toLowerCase().includes(q) ||
          c.host.toLowerCase().includes(q) ||
          c.service_name.toLowerCase().includes(q) ||
          c.username.toLowerCase().includes(q) ||
          (c.group_name?.toLowerCase().includes(q) ?? false)
        );
      })
    : connections;

  $: schemaGroups = buildSchemaGroups(filteredConnections);

  function buildSchemaGroups(conns: ConnectionRecord[]): Map<string, ConnectionRecord[]> {
    const groups = new Map<string, ConnectionRecord[]>();
    for (const c of conns) {
      const key = c.group_name?.trim() || 'Default';
      if (!groups.has(key)) groups.set(key, []);
      groups.get(key)!.push(c);
    }
    return groups;
  }

  async function onOpenPlsql(conn: ConnectionRecord) {
    setBusy(true, `Launching PL/SQL Developer for ${conn.name}…`);
    try {
      await openInPlsqlDeveloper(conn);
      notify(`Opening PL/SQL Developer for '${conn.name}'.`, 'ok');
    } catch (e) {
      notify(`Failed to launch PL/SQL Developer: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function selectConn(conn: ConnectionRecord) {
    form = { ...conn };
    editingId = conn.id;
  }

  function resetForm() {
    form = emptyForm();
    editingId = undefined;
  }

  // ── Handlers ───────────────────────────────────────────────────────
  async function onSave() {
    if (!form.name.trim()) {
      notify('Connection name is required.', 'error');
      return;
    }
    setBusy(true, 'Saving connection…');
    try {
      await saveConnection({ editing_id: editingId, connection: form });
      await onReload();
      notify(`Connection '${form.name}' saved.`, 'ok');
      resetForm();
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onTest() {
    setBusy(true, 'Testing connection…');
    try {
      await testConnection(form);
      notify('Connection test succeeded.', 'ok');
    } catch (e) {
      notify(`Test failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onDelete() {
    if (editingId === undefined) return;
    if (!confirm(`Delete connection '${form.name}'?`)) return;
    setBusy(true, 'Deleting connection…');
    try {
      await deleteConnection(editingId);
      await onReload();
      notify(`Connection '${form.name}' deleted.`, 'ok');
      resetForm();
    } catch (e) {
      notify(`Delete failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function openDeleteAllModal() {
    if (connections.length === 0) return;
    showDeleteAllModal = true;
  }

  async function onConfirmDeleteAll() {
    setBusy(true, 'Deleting all connections…');
    try {
      await deleteAllConnections();
      await onReload();
      resetForm();
      showDeleteAllModal = false;
      notify('All connections deleted.', 'ok');
    } catch (e) {
      notify(`Delete all failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onImport() {
    importError = '';
    try {
      JSON.parse(importJson);
    } catch {
      importError = 'Invalid JSON — expected an array of connection objects.';
      return;
    }
    setBusy(true, 'Importing connections…');
    try {
      const count = await importConnections(importJson);
      await onReload();
      notify(`Imported ${count} connection(s).`, 'ok');
      showImport = false;
      importJson = '';
    } catch (e) {
      importError = `Import failed: ${String(e)}`;
    } finally {
      setBusy(false);
    }
  }

  function openImport() {
    importJson = '';
    importError = '';
    showImport = true;
  }

  async function onExport() {
    const filePath = await save({
      title: 'Export Connections',
      defaultPath: 'connections.json',
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (!filePath) return;
    setBusy(true, 'Exporting connections…');
    try {
      const count = await exportConnections(filePath);
      notify(`Exported ${count} connection(s) to ${filePath}`, 'ok', undefined, filePath);
    } catch (e) {
      notify(`Export failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div style="display:flex;height:100%;overflow:hidden;">

  <!-- ── Left: connection list ── -->
  <div class="conn-list">
    <div class="row" style="margin-bottom:8px;flex-shrink:0;">
      <button class="btn-primary" style="flex:1;font-size:12px;" on:click={resetForm}>+ New</button>
      <button class="btn-secondary" style="font-size:12px;" on:click={openImport}>↓ Import</button>
      <button class="btn-secondary" style="font-size:12px;" on:click={onExport}>↑ Export</button>
    </div>

    <input
      class="conn-search"
      style="margin-bottom:8px;flex-shrink:0;font-size:12px;padding:5px 8px;"
      placeholder="Search connections…"
      bind:value={searchQuery}
    />

    <div class="engine-heading">Oracle</div>

    {#each [...schemaGroups.entries()] as [schema, conns]}
      <div class="conn-group-label">{schema}</div>
      {#each conns as conn}
        <div
          class="conn-item"
        class:selected={editingId === conn.id}
        class:conn-item-broken={conn.password_broken}
          role="button"
          tabindex="0"
          title={conn.password_broken ? 'Password could not be loaded from the OS credential manager. Re-enter and save to fix.' : undefined}
          on:click={() => selectConn(conn)}
          on:keydown={(e) => (e.key === 'Enter' || e.key === ' ') && selectConn(conn)}
        >
          <div class="conn-item-text">
            <div class="conn-item-name">
              {conn.name}
              {#if conn.password_broken}<span class="conn-item-broken-badge">⚠ password missing</span>{/if}
            </div>
            <div class="conn-item-meta">{conn.host}:{conn.port} / {conn.service_name}</div>
          </div>
          <button
            class="conn-item-launch"
            title="Open in PL/SQL Developer"
            disabled={$busy}
            on:click|stopPropagation={() => void onOpenPlsql(conn)}
          >
            <svg xmlns="http://www.w3.org/2000/svg" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><ellipse cx="12" cy="5" rx="9" ry="3"/><path d="M3 5v14a9 3 0 0 0 18 0V5"/><path d="M3 12a9 3 0 0 0 18 0"/></svg>
          </button>
        </div>
      {/each}
    {:else}
      <div class="empty-state">{searchQuery.trim() ? 'No connections match.' : 'No connections yet.'}</div>
    {/each}

    <button
      class="conn-danger-hidden"
      style="margin-top:auto;"
      title="Delete all connections"
      aria-label="Delete all connections"
      disabled={$busy}
      on:click={openDeleteAllModal}
    >●</button>
  </div>

  <!-- ── Right: form ── -->
  <div style="flex:1;padding:16px;overflow-y:auto;display:flex;flex-direction:column;gap:12px;">
    <div class="row" style="flex-shrink:0;">
      <span class="view-title">
      {editingId !== undefined ? `Edit: ${form.name}` : 'New Connection'}
      </span>
      {#if editingId !== undefined}
        <div class="spacer"></div>
        <button class="btn-danger" style="font-size:12px;" on:click={() => void onDelete()} disabled={$busy}>
          🗑 Delete
        </button>
      {/if}
    </div>

    <div class="form-grid">
      <div class="field">
        <span>Name *</span>
        <input bind:value={form.name} placeholder="e.g. PROD_DB" />
      </div>
      <div class="field">
        <span>Group</span>
        <input bind:value={form.group_name} placeholder="e.g. DEV, STAGING, PROD" />
      </div>
      <div class="field">
        <span>Host</span>
        <input bind:value={form.host} placeholder="e.g. 192.168.1.10" />
      </div>
      <div class="field">
        <span>Port</span>
        <input type="number" bind:value={form.port} min="1" max="65535" />
      </div>
      <div class="field">
        <span>Service name</span>
        <input bind:value={form.service_name} placeholder="e.g. ORCL" />
      </div>
      <div class="field">
        <span>Username</span>
        <input bind:value={form.username} placeholder="e.g. APP_USER" />
      </div>
      <div class="field" style="grid-column:span 2;">
        <span>Password</span>
        <div style="position:relative;display:flex;">
          <input
            type={showPassword ? 'text' : 'password'}
            bind:value={form.password}
            style="flex:1;padding-right:32px;"
          />
          <button
            type="button"
            on:click={() => (showPassword = !showPassword)}
            style="position:absolute;right:6px;top:50%;transform:translateY(-50%);background:none;border:none;padding:0;cursor:pointer;color:var(--text-muted);line-height:1;"
            title={showPassword ? 'Hide password' : 'Show password'}
          >
            {#if showPassword}
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94"/><path d="M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
            {:else}
              <svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
            {/if}
          </button>
        </div>
      </div>
    </div>

    <div class="row">
      <button class="btn-primary" on:click={() => void onSave()} disabled={$busy}>
      {editingId !== undefined ? '✓ Update' : '+ Save'}
      </button>
      <button class="btn-secondary" on:click={() => void onTest()} disabled={$busy}>
        Test Connection
      </button>
      <button class="btn-secondary" on:click={resetForm}>Reset</button>
    </div>
  </div>
</div>

<!-- ── Import JSON modal ── -->
{#if showImport}
  <div class="modal-overlay" role="presentation" on:click|self={() => (showImport = false)} on:keydown={(e) => { if (e.key === 'Escape') showImport = false; }}>
    <div class="modal-card" style="width:580px;">
      <div class="modal-header">
        <span class="modal-title">Import Connections (JSON)</span>
        <button class="btn-secondary" style="padding:2px 8px;" on:click={() => (showImport = false)}>✕</button>
      </div>

      <p style="font-size:12px;color:#94a3b8;margin:0 0 10px;">
        Paste a JSON array of connection objects. Connections with an existing name will be overwritten.
      </p>
      <p style="font-size:12px;color:var(--text-muted);margin:0 0 4px;">Expected format (JSON array):</p>
      <pre style="font-size:11px;color:#475569;margin:0 0 8px;font-family:'Consolas',monospace;white-space:pre-wrap;word-break:break-all;background:var(--bg-base);border-radius:4px;padding:6px 8px;">[{"{"}"name":"…","host":"…","port":1521,"service_name":"…","username":"…","password":"…","group_name":"…"{"}"}]</pre>

      <textarea
        rows="10"
        bind:value={importJson}
        placeholder="Paste JSON here…"
        style="width:100%;font-family:'Consolas',monospace;font-size:12px;"
      ></textarea>

      {#if importError}
        <div class="error-box" style="margin-top:8px;">{importError}</div>
      {/if}

      <div class="row" style="justify-content:flex-end;margin-top:12px;">
        <button class="btn-secondary" on:click={() => (showImport = false)}>Cancel</button>
        <button
          class="btn-primary"
          on:click={() => void onImport()}
          disabled={$busy || !importJson.trim()}
        >Import</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── Delete-all confirmation modal ── -->
{#if showDeleteAllModal}
  <Modal width="440px" onClose={() => (showDeleteAllModal = false)}>
    <div style="display:flex;flex-direction:column;gap:14px;padding:4px;">
      <div class="section-title" style="color:var(--btn-danger-bg);">Delete all connections?</div>
      <p style="font-size:13px;color:var(--text-primary);margin:0;">
        This permanently deletes all <strong>{connections.length}</strong> saved connection(s) and their
        passwords from the OS credential manager.
      </p>
      <p style="font-size:12px;color:var(--text-muted);margin:0;">This cannot be undone.</p>
      <div class="row" style="justify-content:flex-end;gap:8px;">
        <button class="btn-secondary" on:click={() => (showDeleteAllModal = false)} disabled={$busy}>Cancel</button>
        <button class="btn-danger" on:click={() => void onConfirmDeleteAll()} disabled={$busy}>
          🗑 Delete All
        </button>
      </div>
    </div>
  </Modal>
{/if}

