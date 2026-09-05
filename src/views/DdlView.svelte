<script lang="ts">
  import { get } from 'svelte/store';
  import type { ConnectionRecord, SchemaObject } from '../types';
  import SqlEditor from '../components/SqlEditor.svelte';
  import Modal from '../components/Modal.svelte';
  import ServerCombobox from '../components/ServerCombobox.svelte';
  import { busy, setBusy, notify } from '../stores/notification';
  import { fetchSchemaObjects, fetchObjectDdl, saveDdlToFolder, openSchemaInVscode } from '../api';
  import { resolvedTheme } from '../hooks/useTheme';
  import {
    selectedServer,
    step,
    objects,
    filterQuery,
    selectedObject,
    generatedDdl,
    collapsedTypes,
  } from '../stores/ddlViewState';

  export let connections: ConnectionRecord[];

  let ddlLoading = false;

  // ── Save to folder modal ──────────────────────────────────────────
  let showSaveModal = false;
  let saveDescription = '';

  $: schema = connections.find(c => c.name === $selectedServer)?.username ?? '';

  function openSaveModal() {
    saveDescription = '';
    showSaveModal = true;
  }

  // Suggested migration description from the selected object: just its name
  // (e.g. the package / function / procedure name).
  function defaultDescription(): string {
    return get(selectedObject)?.name ?? '';
  }

  async function onSaveToFolder() {
    const obj = get(selectedObject);
    const ddl = get(generatedDdl);
    if (!obj || !ddl) return;
    if (!saveDescription.trim()) { notify('Please enter a description.', 'error'); return; }
    showSaveModal = false;
    setBusy(true, 'Saving to folder…');
    try {
      const result = await saveDdlToFolder({
        schema,
        object_name: obj.name,
        object_type: obj.object_type,
        ddl,
        description: saveDescription.trim(),
      });
      const paths = [result.code_path, result.migration_path].filter((p): p is string => !!p);
      const folder = (paths[0] ?? '').replace(/[/\\][^/\\]+$/, '');
      notify(`Saved:\n${paths.join('\n')}`, 'ok', folder);
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  // ── Object type grouping + filtering ─────────────────────────────
  $: filteredObjects = $filterQuery.trim()
    ? $objects.filter(o =>
        o.name.toLowerCase().includes($filterQuery.toLowerCase()) ||
        o.object_type.toLowerCase().includes($filterQuery.toLowerCase()),
      )
    : $objects;

  $: objectGroups = (() => {
    const m = new Map<string, SchemaObject[]>();
    for (const o of filteredObjects) {
      if (!m.has(o.object_type)) m.set(o.object_type, []);
      m.get(o.object_type)!.push(o);
    }
    return m;
  })();

  const TYPE_ORDER = ['TABLE','VIEW','MATERIALIZED VIEW','PROCEDURE','FUNCTION','PACKAGE','PACKAGE BODY','TRIGGER','SEQUENCE','SYNONYM','TYPE','JOB'];
  $: sortedTypes = [...objectGroups.keys()].sort(
    (a, b) => (TYPE_ORDER.indexOf(a) + 1 || 99) - (TYPE_ORDER.indexOf(b) + 1 || 99),
  );

  // ── Handlers ──────────────────────────────────────────────────────
  async function onLoad() {
    if (!get(selectedServer)) { notify('Select a server first.', 'error'); return; }
    setBusy(true, 'Loading schema objects…');
    try {
      const loaded = await fetchSchemaObjects(get(selectedServer));
      objects.set(loaded);
      step.set('objects');
      selectedObject.set(null);
      generatedDdl.set('');
      filterQuery.set('');
      collapsedTypes.set(new Set(TYPE_ORDER));
      notify(`Loaded ${loaded.length} object(s) from ${get(selectedServer)}.`, 'ok');
    } catch (e) {
      notify(`Load failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  async function onSelectObject(obj: SchemaObject) {
    const cur = get(selectedObject);
    if (cur?.name === obj.name && cur?.object_type === obj.object_type) return;
    expandGroupOf(obj);
    selectedObject.set(obj);
    generatedDdl.set('');
    ddlLoading = true;
    try {
      const ddl = await fetchObjectDdl(get(selectedServer), obj.name, obj.object_type);
      generatedDdl.set(ddl);
    } catch (e) {
      generatedDdl.set(`-- Error generating DDL: ${String(e)}`);
    } finally {
      ddlLoading = false;
    }
  }

  function onBack() {
    step.set('pick-server');
    objects.set([]);
    selectedObject.set(null);
    generatedDdl.set('');
  }

  async function onRefresh() {
    if (!get(selectedServer)) return;
    setBusy(true, 'Refreshing schema objects…');
    try {
      const loaded = await fetchSchemaObjects(get(selectedServer));
      objects.set(loaded);
      const cur = get(selectedObject);
      if (cur) {
        const stillExists = loaded.some(o => o.name === cur.name && o.object_type === cur.object_type);
        if (!stillExists) {
          selectedObject.set(null);
          generatedDdl.set('');
        } else {
          generatedDdl.set('');
          ddlLoading = true;
          try {
            const ddl = await fetchObjectDdl(get(selectedServer), cur.name, cur.object_type);
            generatedDdl.set(ddl);
          } catch (e) {
            generatedDdl.set(`-- Error generating DDL: ${String(e)}`);
          } finally {
            ddlLoading = false;
          }
        }
      }
      notify(`Refreshed: ${loaded.length} object(s) from ${get(selectedServer)}.`, 'ok');
    } catch (e) {
      notify(`Refresh failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function copyToClipboard() {
    navigator.clipboard.writeText(get(generatedDdl)).then(() =>
      notify('DDL copied to clipboard.', 'ok'),
    );
  }

  const TYPE_COLORS_DARK: Record<string, string> = {
    TABLE: '#6b9bd1', VIEW: '#9b8cc9', 'MATERIALIZED VIEW': '#b48ead', PROCEDURE: '#5fb88f', FUNCTION: '#5fb88f',
    PACKAGE: '#c9a227', 'PACKAGE BODY': '#b8860b', TRIGGER: '#c76f6f',
    SEQUENCE: '#9a9a9a', SYNONYM: '#9a9a9a', TYPE: '#b87fc9', JOB: '#4fa8a0',
  };
  const TYPE_COLORS_LIGHT: Record<string, string> = {
    TABLE: '#2f6690', VIEW: '#5b4d94', 'MATERIALIZED VIEW': '#8a5a72', PROCEDURE: '#2f7a5c', FUNCTION: '#2f7a5c',
    PACKAGE: '#8a6d1a', 'PACKAGE BODY': '#7a5c0f', TRIGGER: '#943c3c',
    SEQUENCE: '#5c5c5c', SYNONYM: '#5c5c5c', TYPE: '#7a4a8a', JOB: '#2f6b64',
  };
  $: TYPE_COLORS = $resolvedTheme === 'light' ? TYPE_COLORS_LIGHT : TYPE_COLORS_DARK;

  // When filter is active, expand all groups
  $: if ($filterQuery.trim()) collapsedTypes.set(new Set());

  function toggleGroup(type: string) {
    collapsedTypes.update(s => {
      s.has(type) ? s.delete(type) : s.add(type);
      return new Set(s);
    });
  }

  function expandGroupOf(obj: SchemaObject) {
    collapsedTypes.update(s => {
      if (!s.has(obj.object_type)) return s;
      s.delete(obj.object_type);
      return new Set(s);
    });
  }
</script>

<div style="display:flex;flex-direction:column;height:100%;overflow:hidden;padding:10px;gap:10px;">

  {#if $step === 'pick-server'}
    <!-- ── Step 1: pick server ───────────────────────────────────── -->
    <div class="card" style="max-width:480px;display:flex;flex-direction:column;gap:12px;">
      <div class="section-title">Select a server to inspect</div>

      <ServerCombobox
        connections={connections}
        bind:value={$selectedServer}
        placeholder="Search a server…"
      />

      <div>
        <button
          class="btn-primary"
          disabled={!$selectedServer || $busy}
          on:click={() => void onLoad()}
        >→ Load Objects</button>
      </div>
    </div>

  {:else}
    <!-- ── Step 2: object list + DDL ────────────────────────────── -->
    <div style="display:flex;align-items:center;gap:8px;flex-shrink:0;">
      <button class="btn-secondary" style="font-size:12px;" on:click={onBack}>← Back</button>
      <span style="font-size:13px;color:var(--text-muted);">{$selectedServer}</span>
      <span style="font-size:12px;color:var(--text-muted);">({$objects.length} objects)</span>
      <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" title="Refresh schema objects" disabled={$busy} on:click={() => void onRefresh()}>
        🔄 Refresh
      </button>
      {#if schema}
        <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" title="Open schema folder in VS Code" on:click={() => void openSchemaInVscode(schema)}>
          &#x1F5C1; VS Code
        </button>
      {/if}
    </div>

    <div style="display:flex;flex:1;min-height:0;gap:10px;overflow:hidden;">

      <!-- Object list sidebar -->
      <div class="card" style="width:280px;flex-shrink:0;display:flex;flex-direction:column;overflow:hidden;">
        <input
          style="font-size:12px;padding:4px 8px;margin-bottom:8px;flex-shrink:0;"
          placeholder="Filter objects…"
          bind:value={$filterQuery}
        />
        <div style="overflow-y:auto;flex:1;">
          {#each sortedTypes as type}
            {@const group = objectGroups.get(type) ?? []}
            {@const collapsed = $collapsedTypes.has(type)}
            <div style="margin-bottom:2px;">
              <button
                class="ddl-group-header"
                style="color:{TYPE_COLORS[type] ?? 'var(--text-muted)'};"
                on:click={() => toggleGroup(type)}
              >
                <span class="ddl-group-chevron">{collapsed ? '▶' : '▼'}</span>
                {type}
                <span style="font-size:10px;opacity:0.7;margin-left:2px;">({group.length})</span>
              </button>
              {#if !collapsed}
                {#each group as obj}
                  <div
                    class="ddl-object-item"
                    class:ddl-object-item-active={$selectedObject?.name === obj.name && $selectedObject?.object_type === obj.object_type}
                    role="button"
                    tabindex="0"
                    on:click={() => void onSelectObject(obj)}
                    on:keydown={(e) => (e.key === 'Enter' || e.key === ' ') && void onSelectObject(obj)}
                  >
                    {obj.name}
                  </div>
                {/each}
              {/if}
            </div>
          {:else}
            <div class="empty-state">No objects match.</div>
          {/each}
        </div>
      </div>

      <!-- DDL panel -->
      <div class="card" style="flex:1;display:flex;flex-direction:column;overflow:hidden;">
        {#if $selectedObject}
          <div class="row" style="margin-bottom:8px;flex-shrink:0;align-items:center;">
            <span style="font-size:13px;font-weight:600;color:var(--text-accent);">
              {$selectedObject.object_type}: {$selectedObject.name}
            </span>
            <div class="spacer"></div>
            {#if $generatedDdl && !ddlLoading}
              <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={copyToClipboard}>
                📋 Copy
              </button>
              {#if schema}
                <button class="btn-secondary" style="font-size:11px;padding:3px 8px;" on:click={openSaveModal}>
                  💾 Save to Folder
                </button>
              {/if}
            {/if}
          </div>

          {#if ddlLoading}
            <div class="empty-state">Generating DDL…</div>
          {:else if $generatedDdl}
            <SqlEditor value={$generatedDdl} readonly height="100%" />
          {/if}
        {:else}
          <div class="empty-state">Select an object from the list to generate its raw DDL. The idempotent migration script is produced when you save to the folder.</div>
        {/if}
      </div>

    </div>
  {/if}
</div>

{#if showSaveModal}
  <Modal width="440px" onClose={() => { showSaveModal = false; }}>
    <div style="display:flex;flex-direction:column;gap:14px;padding:4px;">
      <div class="section-title">Save to Folder</div>
      <div style="font-size:12px;color:var(--text-muted);">
        Schema: <strong style="color:var(--text-accent);">{schema}</strong> &nbsp;·&nbsp;
        Object: <strong style="color:var(--text-accent);">{$selectedObject?.name}</strong>
      </div>
      <div class="field">
        <span style="font-size:12px;">Description <span style="color:var(--text-muted);font-weight:400;">(double-click to fill default)</span></span>
        <input
          bind:value={saveDescription}
          placeholder="e.g. create_or_replace_DIVISIONS_VW"
          style="font-size:12px;"
          on:dblclick={() => (saveDescription = defaultDescription())}
          on:keydown={(e) => e.key === 'Enter' && void onSaveToFolder()}
        />
      </div>
      <p style="font-size:11px;color:var(--text-muted);margin:0;">
        Migration file (default naming): <code style="color:var(--text-accent);">{new Date().toISOString().slice(2,4)}{(new Date().getMonth()+1).toString().padStart(2,'0')}{new Date().getDate().toString().padStart(2,'0')}_{new Date().getHours().toString().padStart(2,'0')}{new Date().getMinutes().toString().padStart(2,'0')}_{schema.toUpperCase()}${saveDescription || '…'}.&lt;ext&gt;</code>
        <span style="color:var(--text-muted);">(extension follows the DDL extensions setting for {$selectedObject?.object_type})</span>
      </p>
      <div class="row" style="justify-content:flex-end;gap:8px;">
        <button class="btn-secondary" on:click={() => { showSaveModal = false; }}>Cancel</button>
        <button class="btn-primary" disabled={!saveDescription.trim() || $busy} on:click={() => void onSaveToFolder()}>Save</button>
      </div>
    </div>
  </Modal>
{/if}

<style>
  .ddl-group-header {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    padding: 4px 6px;
    font-size: 11px;
    font-weight: 600;
    background: var(--bg-base);
    border: none;
    cursor: pointer;
    border-radius: 4px;
    text-align: left;
    margin-bottom: 1px;
  }
  .ddl-group-header:hover {
    background: var(--nav-btn-hover-bg);
  }
  .ddl-group-chevron {
    font-size: 9px;
    opacity: 0.7;
    flex-shrink: 0;
  }
  .ddl-object-item {
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
    border-radius: 4px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ddl-object-item:hover {
    background: var(--nav-btn-hover-bg);
    color: var(--text-primary);
  }
  .ddl-object-item-active {
    background: var(--nav-btn-active-bg);
    color: var(--text-primary);
    font-weight: 600;
  }
</style>
