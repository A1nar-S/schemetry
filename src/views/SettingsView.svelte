<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import type { ConnectionRecord } from '../types';
  import { DDL_FILE_ENCODINGS, DDL_OBJECT_TYPES, STORAGE_MODES, NAMING_CONVENTIONS, MIGRATION_FOLDER_MODES, LINE_ENDINGS, NAMING_TEMPLATE_PRESETS, NAMING_TEMPLATE_HELP } from '../constants';

  // Every object type needs a concrete (non-undefined) entry so its <select> always
  // matches a real <option> — otherwise the box renders blank until touched.
  function withStorageModeDefaults(modes: Record<string, string>): Record<string, string> {
    return Object.fromEntries(DDL_OBJECT_TYPES.map(t => [t, modes[t] || 'both']));
  }
  import { getSettings, saveSettings } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';
  import { theme, type ThemePreference } from '../hooks/useTheme';
  import SchemaFolderOverridesCard from '../components/SchemaFolderOverridesCard.svelte';
  import McpSettingsCard from '../components/McpSettingsCard.svelte';

  export let connections: ConnectionRecord[] = [];

  let mcpCard: McpSettingsCard | undefined;

  type Tab = 'general' | 'oracle' | 'mcp';
  const THEME_OPTIONS: { id: ThemePreference; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'dark', label: 'Dark' },
    { id: 'light', label: 'Light' },
  ];

  const TABS: { id: Tab; label: string }[] = [
    { id: 'general', label: 'General' },
    { id: 'oracle', label: 'Oracle' },
    { id: 'mcp', label: 'MCP Server' },
  ];
  let tab: Tab = 'general';

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
  let ddlLineEndings = 'keep';
  let namingTemplate = '';
  let plsqlDevPath = '';

  onMount(async () => {
    const s = await getSettings().catch(() => ({
      output_folder: '', client_lib_dir: '', last_query_export_dir: '', schema_root_folder: '',
      ddl_file_encoding: 'utf8', ddl_file_extensions: {}, storage_modes: {}, naming_convention: 'timestamp',
      code_folder_name: 'code', migration_folder_name: 'migration', migration_folder_mode: 'year',
      migration_version_label: '', ddl_line_endings: 'keep', naming_template: '', plsql_dev_path: '',
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
    ddlLineEndings = s.ddl_line_endings || 'keep';
    namingTemplate = s.naming_template;
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
        migrationFolderMode, migrationVersionLabel, ddlLineEndings, namingTemplate.trim(), plsqlDevPath,
      );
      await mcpCard?.saveIfDirty();
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

<div class="settings-scroll">
<div class="settings-view">
  <div class="tabs" role="tablist">
    {#each TABS as t}
      <button
        class="tab"
        class:active={tab === t.id}
        role="tab"
        aria-selected={tab === t.id}
        on:click={() => (tab = t.id)}
      >{t.label}</button>
    {/each}
  </div>

  <!-- Panels stay mounted so unsaved edits survive switching tabs. -->
  <div class="panel" class:active={tab === 'oracle'}>
    <div class="settings-col">
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
  </div>

  <div class="panel" class:active={tab === 'general'}>
    <div class="settings-col">
      <div class="card stack">
        <div class="section-title">Appearance</div>
        <p class="hint">System follows your operating system's light or dark setting.</p>
        <div class="row" role="radiogroup" aria-label="Theme">
          {#each THEME_OPTIONS as o}
            <label style="display:flex;align-items:center;gap:5px;font-size:13px;cursor:pointer;">
              <input type="radio" name="theme" value={o.id} checked={$theme === o.id} on:change={() => theme.set(o.id)} />
              {o.label}
            </label>
          {/each}
        </div>
      </div>

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
            <span>Line endings</span>
            <select bind:value={ddlLineEndings}>
              {#each LINE_ENDINGS as le}
                <option value={le.value}>{le.label}</option>
              {/each}
            </select>
          </div>
        </div>
        <div class="field">
          <span>Migration file name template <span class="hint-inline">(optional — overrides the naming above)</span></span>
          <input bind:value={namingTemplate} placeholder={NAMING_TEMPLATE_PRESETS[namingConvention] ?? NAMING_TEMPLATE_PRESETS.timestamp} />
          <span class="hint">{NAMING_TEMPLATE_HELP}</span>
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

      <SchemaFolderOverridesCard {connections} />
    </div>
  </div>

  <div class="panel" class:active={tab === 'mcp'}>
    <McpSettingsCard bind:this={mcpCard} />
  </div>

  <div class="row">
    <button class="btn-primary" on:click={() => void onSave()} disabled={$busy}>
      Save Settings
    </button>
  </div>
</div>
</div>

<style>
  /* Scrolls at the window edge; content is capped inside it. */
  .settings-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }
  .settings-view {
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 16px;
    max-width: 1180px;
  }
  .tabs {
    display: flex;
    gap: 4px;
    border-bottom: 1px solid var(--border-input);
  }
  .tab {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    padding: 8px 16px;
    margin-bottom: -1px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-muted);
    cursor: pointer;
  }
  .tab:hover { color: var(--text-primary); }
  .tab.active { color: var(--text-link); border-bottom-color: var(--text-link); }
  .panel { display: none; max-width: 760px; }
  .panel.active { display: block; }
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
