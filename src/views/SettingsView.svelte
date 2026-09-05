<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { ConnectionRecord } from '../types';
  import { DDL_FILE_ENCODINGS, DDL_OBJECT_TYPES, STORAGE_MODES, NAMING_CONVENTIONS, MIGRATION_FOLDER_MODES } from '../constants';

  // Every object type needs a concrete (non-undefined) entry so its <select> always
  // matches a real <option> — otherwise the box renders blank until touched.
  function withStorageModeDefaults(modes: Record<string, string>): Record<string, string> {
    return Object.fromEntries(DDL_OBJECT_TYPES.map(t => [t, modes[t] || 'both']));
  }
  import { getSettings, saveSettings } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';
  import SchemaFolderOverridesCard from '../components/SchemaFolderOverridesCard.svelte';

  export let connections: ConnectionRecord[] = [];

  let clientLibDir = '';
  let schemaRootFolder = '';
  let ddlFileEncoding = 'utf8';
  let ddlFileExtensions: Record<string, string> = {};
  let storageModes: Record<string, string> = {};
  let namingConvention = 'timestamp';
  let codeFolderName = 'code';
  let migrationFolderName = 'migration';
  let migrationFolderMode = 'year';
  let migrationVersionLabel = '';
  let plsqlDevPath = '';

  onMount(async () => {
    const s = await getSettings().catch(() => ({
      output_folder: '', client_lib_dir: '', last_query_export_dir: '', schema_root_folder: '',
      ddl_file_encoding: 'utf8', ddl_file_extensions: {}, storage_modes: {}, naming_convention: 'timestamp',
      code_folder_name: 'code', migration_folder_name: 'migration', migration_folder_mode: 'year',
      migration_version_label: '', plsql_dev_path: '',
    }));
    clientLibDir = s.client_lib_dir;
    schemaRootFolder = s.schema_root_folder;
    ddlFileEncoding = s.ddl_file_encoding || 'utf8';
    ddlFileExtensions = { ...s.ddl_file_extensions };
    storageModes = withStorageModeDefaults(s.storage_modes);
    namingConvention = s.naming_convention || 'timestamp';
    codeFolderName = s.code_folder_name || 'code';
    migrationFolderName = s.migration_folder_name || 'migration';
    migrationFolderMode = s.migration_folder_mode || 'year';
    migrationVersionLabel = s.migration_version_label;
    plsqlDevPath = s.plsql_dev_path;
  });

  async function browseClientLibDir() {
    const picked = await open({ title: 'Oracle Instant Client Directory', directory: true, defaultPath: clientLibDir || undefined }) as string | null;
    if (picked) clientLibDir = picked;
  }

  async function browsePlsqlDevPath() {
    const picked = await open({
      title: 'PL/SQL Developer Executable',
      defaultPath: plsqlDevPath || undefined,
      filters: [{ name: 'Executable', extensions: ['exe'] }],
    }) as string | null;
    if (picked) plsqlDevPath = picked;
  }

  async function browseSchemaRootFolder() {
    const picked = await open({ title: 'Schema Root Folder', directory: true, defaultPath: schemaRootFolder || undefined }) as string | null;
    if (picked) schemaRootFolder = picked;
  }

  async function onSave() {
    setBusy(true, 'Saving settings…');
    try {
      // "both" is just the displayed default for an untouched select — only persist
      // entries the user actually changed, so the stored map stays sparse.
      const storageModesToSave = Object.fromEntries(
        Object.entries(storageModes).filter(([, v]) => v && v !== 'both'),
      );
      const res = await saveSettings(
        '', clientLibDir, schemaRootFolder, ddlFileEncoding, ddlFileExtensions,
        storageModesToSave, namingConvention, codeFolderName, migrationFolderName,
        migrationFolderMode, migrationVersionLabel, plsqlDevPath,
      );
      notify(
        res.oracle_client_initialized
          ? 'Settings saved. Oracle client initialized.'
          : 'Settings saved.',
        'ok',
      );
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="settings-view">

  <div class="settings-grid">
    <!-- Left column: database-engine-specific config (Oracle today; Postgres etc. later) -->
    <div class="settings-col">
      <div class="engine-heading">Oracle</div>

      <div class="card stack">
        <div class="section-title">Oracle Instant Client</div>
        <p class="hint">
          Path to the Oracle Instant Client library directory. Required to connect to Oracle databases.
          Example: <code>C:\oracle\instantclient_21_9</code>
        </p>
        <div class="field">
          <span>Client library directory</span>
          <div class="input-row">
            <input bind:value={clientLibDir} placeholder="C:\oracle\instantclient_21_9" />
            <button class="btn-secondary" on:click={() => void browseClientLibDir()}>Browse…</button>
          </div>
        </div>
      </div>

      <div class="card stack">
        <div class="section-title">PL/SQL Developer</div>
        <p class="hint">
          Path to the <code>plsqldev.exe</code> executable, used to launch
          PL/SQL Developer for a connection from the Connections view. Leave empty to auto-detect under Program Files.
        </p>
        <div class="field">
          <span>Executable path</span>
          <div class="input-row">
            <input bind:value={plsqlDevPath} placeholder="C:\Program Files\PLSQL Developer 15\plsqldev.exe" />
            <button class="btn-secondary" on:click={() => void browsePlsqlDevPath()}>Browse…</button>
          </div>
        </div>
      </div>
    </div>

    <!-- Right column: generic config, independent of database engine -->
    <div class="settings-col">
      <div class="engine-heading">General</div>

      <div class="card stack">
        <div class="section-title">Schema Root Folder</div>
        <p class="hint">
          Root folder containing each schema's files as a subfolder.
          DDL objects are saved to
          <code>&lt;folder&gt;/&lt;schema&gt;/{codeFolderName || 'code'}/</code>
          and migration scripts to
          <code>&lt;folder&gt;/&lt;schema&gt;/{migrationFolderName || 'migration'}/</code>.
        </p>
        <div class="field">
          <span>Schema folders root</span>
          <div class="input-row">
            <input bind:value={schemaRootFolder} placeholder="C:\schemas" />
            <button class="btn-secondary" on:click={() => void browseSchemaRootFolder()}>Browse…</button>
          </div>
        </div>
        <div class="field-row">
          <div class="field">
            <span>Code folder name</span>
            <input bind:value={codeFolderName} placeholder="code" />
          </div>
          <div class="field">
            <span>Migration folder name</span>
            <input bind:value={migrationFolderName} placeholder="migration" />
          </div>
        </div>
        <div class="field-row">
          <div class="field">
            <span>DDL file encoding</span>
            <select bind:value={ddlFileEncoding}>
              {#each DDL_FILE_ENCODINGS as enc}
                <option value={enc.value}>{enc.label}</option>
              {/each}
            </select>
          </div>
          <div class="field">
            <span>Migration file naming</span>
            <select bind:value={namingConvention}>
              {#each NAMING_CONVENTIONS as nc}
                <option value={nc.value}>{nc.label}</option>
              {/each}
            </select>
          </div>
        </div>
        <div class="field-row">
          <div class="field">
            <span>Migration folder mode</span>
            <select bind:value={migrationFolderMode}>
              {#each MIGRATION_FOLDER_MODES as m}
                <option value={m.value}>{m.label}</option>
              {/each}
            </select>
          </div>
          {#if migrationFolderMode === 'version'}
            <div class="field">
              <span>Version label <span class="hint-inline">(e.g. 1.4)</span></span>
              <input bind:value={migrationVersionLabel} placeholder="1.4" />
            </div>
          {/if}
        </div>
      </div>

      <div class="card stack">
        <div class="section-title">DDL File Settings</div>
        <p class="hint">Per object type — applies to both code and migration files.</p>
        <div class="field">
          <span>File extensions</span>
          <div class="ext-grid">
            {#each DDL_OBJECT_TYPES as type}
              <label class="ext-cell">
                <span>{type}</span>
                <input bind:value={ddlFileExtensions[type]} placeholder="sql" />
              </label>
            {/each}
          </div>
        </div>
        <div class="field">
          <span>Storage mode <span class="hint-inline">(what gets written to the folder)</span></span>
          <div class="mode-grid">
            {#each DDL_OBJECT_TYPES as type}
              <label class="mode-cell">
                <span>{type}</span>
                <select bind:value={storageModes[type]}>
                  {#each STORAGE_MODES as mode}
                    <option value={mode.value}>{mode.label}</option>
                  {/each}
                </select>
              </label>
            {/each}
          </div>
        </div>
      </div>
    </div>
  </div>

  <SchemaFolderOverridesCard {connections} />

  <div class="row">
    <button class="btn-primary" on:click={() => void onSave()} disabled={$busy}>
      Save Settings
    </button>
  </div>
</div>

<style>
  .settings-view {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 1180px;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .settings-grid {
    display: grid;
    grid-template-columns: minmax(280px, 400px) 1fr;
    gap: 16px 24px;
    align-items: start;
  }
  @media (max-width: 860px) {
    .settings-grid {
      grid-template-columns: 1fr;
    }
  }
  .settings-col {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-width: 0;
  }
  .input-row {
    display: flex;
    gap: 6px;
  }
  .input-row input {
    flex: 1;
  }
  .input-row .btn-secondary {
    white-space: nowrap;
  }
  .field select {
    font-size: 13px;
    padding: 6px 10px;
    width: 100%;
  }
  .field-row {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .field-row .field {
    flex: 1;
    min-width: 160px;
  }
  .hint-inline {
    font-weight: 400;
    color: var(--text-muted);
    font-size: 11px;
  }
  .ext-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
    gap: 6px 10px;
  }
  .ext-cell {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .ext-cell input {
    width: 60px;
    flex-shrink: 0;
    text-align: center;
    font-size: 12px;
    padding: 4px 6px;
  }
  .mode-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 6px 10px;
  }
  .mode-cell {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .mode-cell select {
    width: 120px;
    flex-shrink: 0;
    font-size: 11px;
    padding: 4px 6px;
    text-overflow: ellipsis;
  }
</style>
