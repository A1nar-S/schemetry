<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { ConnectionRecord, SchemaFolderOverride } from '../types';
  import { DDL_FILE_ENCODINGS, DDL_OBJECT_TYPES, STORAGE_MODES, NAMING_CONVENTIONS, MIGRATION_FOLDER_MODES } from '../constants';
  import { listSchemaFolderOverrides, saveSchemaFolderOverride, deleteSchemaFolderOverride } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';
  import ServerCombobox from './ServerCombobox.svelte';
  import Modal from './Modal.svelte';

  export let connections: ConnectionRecord[] = [];

  let overrides: SchemaFolderOverride[] = [];
  let loaded = false;

  // Reuses ServerCombobox (the DDL view's server picker) over the distinct schema
  // names seen across configured connections — the Oracle username for Oracle
  // connections, `pg_schema` (defaulting to `public`) for Postgres ones. Lowercased
  // and deduplicated case-insensitively to match how schema_name is normalized on save.
  let schemaOptions: ConnectionRecord[] = [];
  $: schemaOptions = [...new Set(
      connections
        .map(c => (c.db_type === 'postgres' ? (c.pg_schema?.trim() || 'public') : c.username.trim()).toLowerCase())
        .filter(Boolean),
    )]
    .sort()
    .map(name => ({
      id: 0, name, db_type: 'oracle', host: '', port: 1521, service_name: '',
      username: name, password: '', group_name: '', pg_schema: '',
    }));

  onMount(async () => {
    await reload();
  });

  async function reload() {
    try {
      overrides = await listSchemaFolderOverrides();
    } catch (e) {
      notify(`Failed to load schema folder overrides: ${String(e)}`, 'error');
    } finally {
      loaded = true;
    }
  }

  async function persist(item: SchemaFolderOverride) {
    if (
      !item.schema_name.trim() ||
      (!item.folder_path.trim() &&
        !item.encoding.trim() &&
        !item.extensions.trim() &&
        !item.storage_modes.trim() &&
        !item.naming_convention.trim() &&
        !item.code_folder_name.trim() &&
        !item.migration_folder_name.trim() &&
        !item.migration_folder_mode.trim() &&
        !item.migration_version_label.trim())
    )
      return;
    setBusy(true, 'Saving schema folder override…');
    try {
      const saved = await saveSchemaFolderOverride(item);
      overrides = overrides.map(o => (o === item ? saved : o));
    } catch (e) {
      notify(`Failed to save schema folder override: ${String(e)}`, 'error');
      await reload();
    } finally {
      setBusy(false);
    }
  }

  function addOverride() {
    overrides = [
      ...overrides,
      {
        id: 0, schema_name: '', folder_path: '', encoding: '', extensions: '', storage_modes: '',
        naming_convention: '', code_folder_name: '', migration_folder_name: '',
        migration_folder_mode: '', migration_version_label: '',
      },
    ];
  }

  // ── Per-schema extension overrides modal ───────────────────────
  let extModalItem: SchemaFolderOverride | null = null;
  let extModalMap: Record<string, string> = {};

  function extensionOverrideCount(item: SchemaFolderOverride): number {
    if (!item.extensions.trim()) return 0;
    try {
      const map = JSON.parse(item.extensions) as Record<string, string>;
      return Object.values(map).filter(v => v.trim()).length;
    } catch {
      return 0;
    }
  }

  function openExtModal(item: SchemaFolderOverride) {
    let parsed: Record<string, string> = {};
    if (item.extensions.trim()) {
      try { parsed = JSON.parse(item.extensions); } catch { parsed = {}; }
    }
    extModalMap = Object.fromEntries(DDL_OBJECT_TYPES.map(t => [t, parsed[t] ?? '']));
    extModalItem = item;
  }

  function closeExtModal() {
    extModalItem = null;
  }

  async function saveExtModal() {
    if (!extModalItem) return;
    const item = extModalItem;
    const cleaned = Object.fromEntries(
      Object.entries(extModalMap).filter(([, v]) => v.trim()),
    );
    item.extensions = Object.keys(cleaned).length ? JSON.stringify(cleaned) : '';
    overrides = overrides;
    closeExtModal();
    await persist(item);
  }

  // ── Per-schema storage-mode overrides modal ───────────────────────
  let storageModalItem: SchemaFolderOverride | null = null;
  let storageModalMap: Record<string, string> = {};

  function storageOverrideCount(item: SchemaFolderOverride): number {
    if (!item.storage_modes.trim()) return 0;
    try {
      const map = JSON.parse(item.storage_modes) as Record<string, string>;
      return Object.values(map).filter(v => v.trim()).length;
    } catch {
      return 0;
    }
  }

  function openStorageModal(item: SchemaFolderOverride) {
    let parsed: Record<string, string> = {};
    if (item.storage_modes.trim()) {
      try { parsed = JSON.parse(item.storage_modes); } catch { parsed = {}; }
    }
    storageModalMap = Object.fromEntries(DDL_OBJECT_TYPES.map(t => [t, parsed[t] ?? '']));
    storageModalItem = item;
  }

  function closeStorageModal() {
    storageModalItem = null;
  }

  async function saveStorageModal() {
    if (!storageModalItem) return;
    const item = storageModalItem;
    const cleaned = Object.fromEntries(
      Object.entries(storageModalMap).filter(([, v]) => v.trim()),
    );
    item.storage_modes = Object.keys(cleaned).length ? JSON.stringify(cleaned) : '';
    overrides = overrides;
    closeStorageModal();
    await persist(item);
  }

  // ── Per-schema migration settings modal (folder names, naming convention, folder mode/label) ──
  let migrationModalItem: SchemaFolderOverride | null = null;
  let migrationModalCodeFolder = '';
  let migrationModalMigrationFolder = '';
  let migrationModalNaming = '';
  let migrationModalFolderMode = '';
  let migrationModalVersionLabel = '';

  function migrationOverrideCount(item: SchemaFolderOverride): number {
    return [
      item.code_folder_name,
      item.migration_folder_name,
      item.naming_convention,
      item.migration_folder_mode,
      item.migration_version_label,
    ].filter(v => v.trim()).length;
  }

  function openMigrationModal(item: SchemaFolderOverride) {
    migrationModalCodeFolder = item.code_folder_name;
    migrationModalMigrationFolder = item.migration_folder_name;
    migrationModalNaming = item.naming_convention;
    migrationModalFolderMode = item.migration_folder_mode;
    migrationModalVersionLabel = item.migration_version_label;
    migrationModalItem = item;
  }

  function closeMigrationModal() {
    migrationModalItem = null;
  }

  async function saveMigrationModal() {
    if (!migrationModalItem) return;
    const item = migrationModalItem;
    item.code_folder_name = migrationModalCodeFolder.trim();
    item.migration_folder_name = migrationModalMigrationFolder.trim();
    item.naming_convention = migrationModalNaming;
    item.migration_folder_mode = migrationModalFolderMode;
    item.migration_version_label = migrationModalFolderMode === 'version' ? migrationModalVersionLabel.trim() : '';
    overrides = overrides;
    closeMigrationModal();
    await persist(item);
  }

  async function browseFolder(item: SchemaFolderOverride) {
    const picked = await open({ title: 'Schema Folder', directory: true, defaultPath: item.folder_path || undefined }) as string | null;
    if (picked) {
      item.folder_path = picked;
      overrides = overrides;
      void persist(item);
    }
  }

  async function onDelete(item: SchemaFolderOverride) {
    if (item.id === 0) {
      overrides = overrides.filter(o => o !== item);
      return;
    }
    setBusy(true, 'Deleting schema folder override…');
    try {
      await deleteSchemaFolderOverride(item.id);
      overrides = overrides.filter(o => o.id !== item.id);
    } catch (e) {
      notify(`Failed to delete schema folder override: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="card stack">
  <div class="section-title">Schema Folder Overrides</div>
  <p class="hint">
    Per-schema overrides for schemas that don't fit the defaults above: repository folder,
    encoding, file extensions, storage modes, and migration settings (naming, folder
    names, folder mode) — each left blank falls back to the default.
  </p>

  {#if loaded && overrides.length === 0}
    <div class="hint">No overrides — all schemas use the defaults.</div>
  {/if}

  {#each overrides as item (item)}
    <div class="override-row">
      <div class="override-schema">
        <ServerCombobox
          connections={schemaOptions}
          bind:value={item.schema_name}
          placeholder="Schema name"
          onChange={() => void persist(item)}
        />
      </div>
      <div class="override-folder">
        <input
          bind:value={item.folder_path}
          placeholder="Default folder"
          on:blur={() => void persist(item)}
        />
        <button class="btn-secondary" on:click={() => void browseFolder(item)}>Browse…</button>
      </div>
      <select bind:value={item.encoding} on:change={() => void persist(item)}>
        <option value="">Default encoding</option>
        {#each DDL_FILE_ENCODINGS as enc}
          <option value={enc.value}>{enc.label}</option>
        {/each}
      </select>
      <button class="btn-secondary ext-override-btn" on:click={() => openExtModal(item)} title="Per-object-type DDL file extension overrides for this schema">
        📄 Extensions{#if extensionOverrideCount(item)} <span class="ext-override-badge">{extensionOverrideCount(item)}</span>{/if}
      </button>
      <button class="btn-secondary ext-override-btn" on:click={() => openStorageModal(item)} title="Per-object-type DDL storage mode overrides for this schema">
        🗄 Storage{#if storageOverrideCount(item)} <span class="ext-override-badge">{storageOverrideCount(item)}</span>{/if}
      </button>
      <button class="btn-secondary ext-override-btn" on:click={() => openMigrationModal(item)} title="Code/migration folder names, file naming, and migration folder overrides for this schema">
        🚀 Migration{#if migrationOverrideCount(item)} <span class="ext-override-badge">{migrationOverrideCount(item)}</span>{/if}
      </button>
      <button class="btn-danger" style="padding:4px 8px;font-size:12px;" on:click={() => void onDelete(item)} disabled={$busy}>
        🗑
      </button>
    </div>
  {/each}

  <button class="btn-secondary" style="align-self:flex-start;font-size:12px;" on:click={addOverride}>+ Add Override</button>
</div>

{#if extModalItem}
  <Modal width="600px" onClose={closeExtModal}>
    <div style="display:flex;flex-direction:column;gap:14px;padding:4px;">
      <div class="section-title">DDL Extensions{#if extModalItem.schema_name} — {extModalItem.schema_name}{/if}</div>
      <p class="hint" style="margin:0;">
        Overrides the default extension per object type for this schema only. Leave a field
        blank to keep using the global default.
      </p>
      <div class="ext-grid">
        {#each DDL_OBJECT_TYPES as type}
          <label class="ext-cell">
            <span>{type}</span>
            <input bind:value={extModalMap[type]} placeholder="default" />
          </label>
        {/each}
      </div>
      <div class="row" style="justify-content:flex-end;gap:8px;">
        <button class="btn-secondary" on:click={closeExtModal}>Cancel</button>
        <button class="btn-primary" disabled={$busy} on:click={() => void saveExtModal()}>Save</button>
      </div>
    </div>
  </Modal>
{/if}

{#if storageModalItem}
  <Modal width="600px" onClose={closeStorageModal}>
    <div style="display:flex;flex-direction:column;gap:14px;padding:4px;">
      <div class="section-title">Storage Modes{#if storageModalItem.schema_name} — {storageModalItem.schema_name}{/if}</div>
      <p class="hint" style="margin:0;">
        Overrides what gets written to the schema folder per object type for this schema only. Leave a field
        blank to keep using the global default.
      </p>
      <div class="ext-grid">
        {#each DDL_OBJECT_TYPES as type}
          <label class="ext-cell">
            <span>{type}</span>
            <select bind:value={storageModalMap[type]}>
              <option value="">default</option>
              {#each STORAGE_MODES as mode}
                <option value={mode.value}>{mode.label}</option>
              {/each}
            </select>
          </label>
        {/each}
      </div>
      <div class="row" style="justify-content:flex-end;gap:8px;">
        <button class="btn-secondary" on:click={closeStorageModal}>Cancel</button>
        <button class="btn-primary" disabled={$busy} on:click={() => void saveStorageModal()}>Save</button>
      </div>
    </div>
  </Modal>
{/if}

{#if migrationModalItem}
  <Modal width="420px" onClose={closeMigrationModal}>
    <div style="display:flex;flex-direction:column;gap:14px;padding:4px;">
      <div class="section-title">Migration Settings{#if migrationModalItem.schema_name} — {migrationModalItem.schema_name}{/if}</div>
      <p class="hint" style="margin:0;">
        Overrides the code/migration folder names, migration file naming, and the
        migration subfolder for this schema only. Leave a field on "Default" (or blank) to
        keep using the global setting.
      </p>
      <label class="migration-field">
        <span>Code folder name</span>
        <input bind:value={migrationModalCodeFolder} placeholder="Default (code)" />
      </label>
      <label class="migration-field">
        <span>Migration folder name</span>
        <input bind:value={migrationModalMigrationFolder} placeholder="Default (migration)" />
      </label>
      <label class="migration-field">
        <span>Migration file naming</span>
        <select bind:value={migrationModalNaming}>
          <option value="">Default naming</option>
          {#each NAMING_CONVENTIONS as nc}
            <option value={nc.value}>{nc.label}</option>
          {/each}
        </select>
      </label>
      <label class="migration-field">
        <span>Migration folder mode</span>
        <select bind:value={migrationModalFolderMode}>
          <option value="">Default mode</option>
          {#each MIGRATION_FOLDER_MODES as m}
            <option value={m.value}>{m.label}</option>
          {/each}
        </select>
      </label>
      {#if migrationModalFolderMode === 'version'}
        <label class="migration-field">
          <span>Version label</span>
          <input bind:value={migrationModalVersionLabel} placeholder="1.4" />
        </label>
      {/if}
      <div class="row" style="justify-content:flex-end;gap:8px;">
        <button class="btn-secondary" on:click={closeMigrationModal}>Cancel</button>
        <button class="btn-primary" disabled={$busy} on:click={() => void saveMigrationModal()}>Save</button>
      </div>
    </div>
  </Modal>
{/if}

<style>
  .override-row {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .override-schema {
    width: 140px;
    flex-shrink: 0;
  }
  .override-folder {
    display: flex;
    gap: 6px;
    flex: 1;
    min-width: 200px;
  }
  .override-folder input {
    flex: 1;
  }
  .override-folder button {
    white-space: nowrap;
  }
  .override-row select {
    font-size: 13px;
    padding: 6px 10px;
    flex-shrink: 0;
    width: 150px;
    text-overflow: ellipsis;
  }
  .migration-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-muted);
  }
  .ext-override-btn {
    flex-shrink: 0;
    white-space: nowrap;
    padding: 7px 14px;
    font-size: 13px;
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .ext-override-badge {
    background: var(--color-ember, #b54f2e);
    color: #fff;
    border-radius: 10px;
    padding: 1px 7px;
    font-size: 11px;
    font-weight: 700;
  }
  .ext-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 10px 16px;
  }
  .ext-cell {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    font-size: 13px;
    color: var(--text-muted);
  }
  .ext-cell input {
    width: 90px;
    flex-shrink: 0;
    text-align: center;
    font-size: 13px;
    padding: 5px 6px;
  }
  .ext-cell select {
    width: 130px;
    flex-shrink: 0;
    font-size: 12px;
    padding: 5px 6px;
    text-overflow: ellipsis;
  }
</style>
