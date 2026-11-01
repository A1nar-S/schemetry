<script lang="ts">
  import Icon from './Icon.svelte';
  import Modal from './Modal.svelte';
  import VirtualTable from './VirtualTable.svelte';
  import type { VCol } from './VirtualTable.svelte';
  import { onMount } from 'svelte';
  import type { ConnectionRecord, McpPolicy, McpPreview, SchemaObject } from '../types';
  import { fetchSchemaObjects, getMcpPolicy, mcpPreview, saveMcpPolicy } from '../api';
  import { DDL_OBJECT_TYPES } from '../constants';
  import { busy, setBusy, notify } from '../stores/notification';

  export let connection: ConnectionRecord;
  export let onClose: () => void;
  // Called after a successful save so the caller can refresh its policy list.
  export let onSaved: () => void | Promise<void> = () => {};

  const PATTERN_RE = /^[A-Za-z0-9_$#*?-]+$/;
  const TABLE_TYPES = new Set(['TABLE', 'VIEW', 'MATERIALIZED VIEW']);
  const PICKER_LIMIT = 200;

  let policy: McpPolicy | null = null;
  let baseline = '';
  let accountPassword = '';
  let clearPassword = false;
  let showPassword = false;

  $: dirty = policy !== null && (JSON.stringify(policy) !== baseline || accountPassword !== '' || clearPassword);
  $: isPostgres = connection.db_type === 'postgres';

  onMount(async () => {
    setBusy(true, 'Loading MCP policy…');
    try {
      setPolicy(await getMcpPolicy(connection.id));
    } catch (e) {
      notify(`Failed to load MCP policy: ${String(e)}`, 'error');
      onClose();
    } finally {
      setBusy(false);
    }
  });

  function setPolicy(p: McpPolicy) {
    policy = p;
    baseline = JSON.stringify(p);
    accountPassword = '';
    clearPassword = false;
  }

  // ── Rules ─────────────────────────────────────────────────────────
  function addRule(action: 'allow' | 'deny' = 'allow', table = '') {
    if (!policy) return;
    policy.table_rules = [...policy.table_rules, { action, table, column: '' }];
  }

  function removeRule(index: number) {
    if (!policy) return;
    policy.table_rules = policy.table_rules.filter((_, i) => i !== index);
  }

  function addDenyObject() {
    if (!policy) return;
    policy.object_deny = [...policy.object_deny, { object_type: '*', name: '' }];
  }

  function removeDenyObject(index: number) {
    if (!policy) return;
    policy.object_deny = policy.object_deny.filter((_, i) => i !== index);
  }

  // ── Table picker ──────────────────────────────────────────────────
  let pickerObjects: SchemaObject[] | null = null;
  let pickerQuery = '';

  async function loadPicker() {
    setBusy(true, `Loading tables from ${connection.name}…`);
    try {
      const all = await fetchSchemaObjects(connection.id);
      pickerObjects = all
        .filter(o => TABLE_TYPES.has(o.object_type.toUpperCase()))
        .sort((a, b) => a.name.localeCompare(b.name));
    } catch (e) {
      notify(`Failed to load tables: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  $: pickerMatches = pickerObjects
    ? pickerObjects.filter(o => o.name.toLowerCase().includes(pickerQuery.trim().toLowerCase()))
    : [];

  function isAllowed(name: string): boolean {
    return !!policy?.table_rules.some(r => r.action === 'allow' && r.column === '' && r.table === name);
  }

  function pickTable(name: string) {
    if (!policy || isAllowed(name)) return;
    addRule('allow', name);
  }

  // ── Validation + save ─────────────────────────────────────────────
  function validate(p: McpPolicy): string | null {
    for (const [i, r] of p.table_rules.entries()) {
      const n = i + 1;
      if (!r.table.trim()) return `Table rule ${n}: table pattern is required.`;
      if (!PATTERN_RE.test(r.table)) return `Table rule ${n}: table pattern may only contain letters, digits and _ $ # * ? -`;
      if (r.column !== '' && !PATTERN_RE.test(r.column)) return `Table rule ${n}: column pattern may only contain letters, digits and _ $ # * ? -`;
    }
    for (const [i, r] of p.object_deny.entries()) {
      const n = i + 1;
      if (!r.name.trim()) return `Hidden object ${n}: name pattern is required.`;
      if (!PATTERN_RE.test(r.name)) return `Hidden object ${n}: name pattern may only contain letters, digits and _ $ # * ? -`;
    }
    if (!Number.isInteger(p.max_rows) || p.max_rows < 1 || p.max_rows > 10000) {
      return 'Max rows must be a whole number between 1 and 10000.';
    }
    if (!Number.isInteger(p.query_timeout_secs) || p.query_timeout_secs < 1 || p.query_timeout_secs > 600) {
      return 'Query timeout must be a whole number of seconds between 1 and 600.';
    }
    return null;
  }

  // Returns whether the policy was saved.
  async function persist(): Promise<boolean> {
    if (!policy) return false;
    const toSave: McpPolicy = {
      ...policy,
      table_rules: policy.table_rules.map(r => ({ ...r, table: r.table.trim(), column: r.column.trim() })),
      object_deny: policy.object_deny.map(r => ({ ...r, name: r.name.trim() })),
      mcp_username: policy.mcp_username.trim(),
    };
    const problem = validate(toSave);
    if (problem) {
      notify(problem, 'error');
      return false;
    }
    setBusy(true, 'Saving MCP policy…');
    try {
      const saved = await saveMcpPolicy({
        policy: toSave,
        account_password: accountPassword || null,
        clear_account_password: clearPassword,
      });
      setPolicy(saved);
      await onSaved();
      notify(`MCP policy for '${connection.name}' saved.`, 'ok');
      return true;
    } catch (e) {
      notify(`Save failed: ${String(e)}`, 'error');
      return false;
    } finally {
      setBusy(false);
    }
  }

  // ── Test as MCP ───────────────────────────────────────────────────
  let preview: McpPreview | null = null;

  const previewCols: VCol[] = [
    { key: 'name', header: 'Object', flex: 1, minWidth: 200 },
    { key: 'object_type', header: 'Type', width: 150 },
    { key: 'data_access', header: 'Data access', width: 120 },
  ];

  function previewCellClass(row: Record<string, unknown>, key: string): string {
    if (key !== 'data_access') return '';
    switch (row.data_access) {
      case 'allowed': return 'vt-cell-ok';
      case 'restricted': return 'vt-cell-warn';
      case 'denied': return 'vt-cell-danger';
      default: return 'vt-cell-muted';
    }
  }

  async function onTest() {
    if (!policy) return;
    if (dirty) {
      if (!confirm('The preview uses the saved policy. Save your changes and test?')) return;
      if (!(await persist())) return;
    }
    setBusy(true, 'Testing as MCP agent…');
    try {
      preview = await mcpPreview(connection.id);
    } catch (e) {
      notify(`Preview failed: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }

  function onClosing() {
    if (dirty && !confirm('Discard unsaved MCP policy changes?')) return;
    onClose();
  }
</script>

<Modal width="900px" onClose={onClosing}>
  <div class="modal-header">
    <span class="modal-title">MCP access — {connection.name}</span>
    <button class="btn-secondary" style="padding:2px 8px;" on:click={onClosing} title="Close"><Icon name="x" /></button>
  </div>

  {#if policy}
    <div class="stack">
      <!-- (a) Master toggle -->
      <div class="card stack">
        <label class="check-row check-row-lg">
          <input type="checkbox" bind:checked={policy.mcp_enabled} />
          Expose this connection to MCP agents
        </label>
        <p class="hint">
          Off by default. While off, agents cannot see or use this connection at all. Every agent call is recorded in the audit log.
        </p>
      </div>

      <fieldset class="policy-body" disabled={!policy.mcp_enabled}>
        <!-- (b) Capabilities -->
        <div class="card stack">
          <div class="section-title">Capabilities</div>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_schema_list} /> List schema objects</label>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_ddl_read} /> Read DDL</label>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_invalid_objects} /> Read invalid objects &amp; compile errors (Oracle)</label>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_save_scripts} /> Save migration / code files</label>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_compare_fix} /> Compare schemas &amp; generate fix scripts</label>
          <label class="check-row"><input type="checkbox" bind:checked={policy.allow_compare_object} /> Compare a local file with an object's DDL (needs Read DDL)</label>
        </div>

        <!-- (c) Data access -->
        <div class="card stack">
          <div class="section-title">Table data access</div>
          <label class="check-row">
            <input type="radio" name="mcp-data-access" value="none" bind:group={policy.data_access} />
            None — agents cannot read table data
          </label>
          <label class="check-row">
            <input type="radio" name="mcp-data-access" value="allowlist" bind:group={policy.data_access} />
            Allowlist — agents can read only the tables and columns allowed below
          </label>

          {#if policy.data_access === 'allowlist'}
            <p class="hint">
              <strong>Deny always wins</strong> over allow. Patterns support the wildcards <code>*</code> and <code>?</code>
              (allowed characters: letters, digits, <code>_ $ # * ?</code> and <code>-</code>). A blank column means the whole table.
              {isPostgres
                ? 'On PostgreSQL, allow patterns are case-sensitive.'
                : 'On Oracle, allow patterns are case-insensitive.'}
            </p>

            {#each policy.table_rules as rule, i}
              <div class="rule-row">
                <select bind:value={rule.action} aria-label="Rule action">
                  <option value="allow">Allow</option>
                  <option value="deny">Deny</option>
                </select>
                <input class="rule-table" bind:value={rule.table} placeholder="Table pattern, e.g. ORDERS*" aria-label="Table pattern" />
                <input class="rule-column" bind:value={rule.column} placeholder="Column (optional)" aria-label="Column pattern" />
                <button class="btn-danger" style="padding:4px 8px;font-size:12px;" title="Remove rule" aria-label="Remove rule" on:click={() => removeRule(i)}>
                  <Icon name="trash" />
                </button>
              </div>
            {:else}
              <div class="hint">No rules yet — with an allowlist and no allow rules, no table data is readable.</div>
            {/each}
            <div class="row">
              <button class="btn-secondary" style="font-size:12px;" on:click={() => addRule()}>+ Add rule</button>
              <button class="btn-secondary" style="font-size:12px;" on:click={() => void loadPicker()} disabled={$busy}>
                <Icon name="database" /> {pickerObjects ? 'Reload tables' : 'Pick from database tables…'}
              </button>
            </div>

            {#if pickerObjects}
              <div class="picker">
                <input
                  class="picker-search"
                  bind:value={pickerQuery}
                  placeholder="Search {pickerObjects.length} tables / views…"
                  aria-label="Search tables"
                />
                <div class="picker-list">
                  {#each pickerMatches.slice(0, PICKER_LIMIT) as obj (obj.object_type + '\0' + obj.name)}
                    <button
                      class="picker-item"
                      title={isAllowed(obj.name) ? 'Already allowed' : `Add an allow rule for ${obj.name}`}
                      disabled={isAllowed(obj.name)}
                      on:click={() => pickTable(obj.name)}
                    >
                      <Icon name={isAllowed(obj.name) ? 'check' : 'plus'} size={12} />
                      <span class="picker-name">{obj.name}</span>
                      <span class="picker-type">{obj.object_type}</span>
                    </button>
                  {:else}
                    <div class="hint" style="padding:6px;">No tables match.</div>
                  {/each}
                  {#if pickerMatches.length > PICKER_LIMIT}
                    <div class="hint" style="padding:6px;">Showing the first {PICKER_LIMIT} of {pickerMatches.length} — refine the search.</div>
                  {/if}
                </div>
              </div>
            {/if}
          {/if}
        </div>

        <!-- (d) Raw SQL -->
        <div class="card stack">
          <div class="section-title">Raw SQL</div>
          <label class="check-row">
            <input type="checkbox" bind:checked={policy.allow_raw_sql} disabled={policy.data_access !== 'allowlist'} />
            Allow raw SQL (<code>run_query</code>)
          </label>
          <p class="hint">
            Only meaningful with an allowlist{policy.data_access !== 'allowlist' ? ' — enable the allowlist above first' : ''}.
            Restricting raw SQL is <strong>best-effort, not a hard guarantee</strong>; use a dedicated read-only database account.
          </p>
          {#if policy.allow_raw_sql && policy.data_access === 'allowlist'}
            {#if policy.mcp_username.trim()}
              <div class="mcp-notice mcp-notice-warn">
                <Icon name="alert-triangle" size={15} />
                <span>Raw SQL is best-effort, not a hard guarantee. Make sure the dedicated account <code>{policy.mcp_username.trim()}</code> has SELECT only on the allowed tables.</span>
              </div>
            {:else}
              <div class="mcp-notice mcp-notice-danger">
                <Icon name="alert-triangle" size={15} />
                <span>
                  Raw SQL is on and no dedicated MCP database account is set — queries would run with this connection's own credentials.
                  This is best-effort, not a hard guarantee. Set a dedicated read-only account below.
                </span>
              </div>
            {/if}
          {/if}
        </div>

        <!-- (e) Hidden objects -->
        <div class="card stack">
          <div class="section-title">Hidden objects (DDL)</div>
          <p class="hint">Objects matching these rules are hidden from agents in schema listings, DDL and compare results.</p>
          {#each policy.object_deny as rule, i}
            <div class="rule-row">
              <select bind:value={rule.object_type} aria-label="Object type">
                <option value="*">Any type (*)</option>
                {#each DDL_OBJECT_TYPES as t}
                  <option value={t}>{t}</option>
                {/each}
              </select>
              <input class="rule-table" bind:value={rule.name} placeholder="Name pattern, e.g. SECRET_*" aria-label="Object name pattern" />
              <button class="btn-danger" style="padding:4px 8px;font-size:12px;" title="Remove rule" aria-label="Remove rule" on:click={() => removeDenyObject(i)}>
                <Icon name="trash" />
              </button>
            </div>
          {/each}
          <button class="btn-secondary" style="align-self:flex-start;font-size:12px;" on:click={addDenyObject}>+ Add hidden object</button>
        </div>

        <!-- (f) Limits -->
        <div class="card stack">
          <div class="section-title">Limits</div>
          <div class="field-row">
            <div class="field">
              <span>Max rows <span class="hint-inline">(1–10000)</span></span>
              <input type="number" min="1" max="10000" step="1" bind:value={policy.max_rows} />
            </div>
            <div class="field">
              <span>Query timeout, seconds <span class="hint-inline">(1–600)</span></span>
              <input type="number" min="1" max="600" step="1" bind:value={policy.query_timeout_secs} />
            </div>
          </div>
        </div>

        <!-- (g) Dedicated account -->
        <div class="card stack">
          <div class="section-title">Dedicated MCP database account <span class="hint-inline">(optional)</span></div>
          <p class="hint">
            Used only for table-data reads (<code>query_table</code>, <code>run_query</code>). It should have SELECT only on the allowed tables.
            Leave empty to use this connection's own credentials.
          </p>
          <div class="field-row">
            <div class="field">
              <span>Username</span>
              <input bind:value={policy.mcp_username} placeholder="e.g. MCP_READONLY" autocomplete="off" />
            </div>
            <div class="field">
              <span>Password</span>
              <div class="pw-wrap">
                <input
                  type={showPassword ? 'text' : 'password'}
                  bind:value={accountPassword}
                  on:input={() => (clearPassword = false)}
                  placeholder={policy.mcp_password_set && !clearPassword ? 'Leave blank to keep the stored password' : 'Password'}
                  autocomplete="new-password"
                />
                <button
                  type="button"
                  class="pw-toggle"
                  on:click={() => (showPassword = !showPassword)}
                  title={showPassword ? 'Hide password' : 'Show password'}
                  aria-label={showPassword ? 'Hide password' : 'Show password'}
                >{showPassword ? 'Hide' : 'Show'}</button>
              </div>
            </div>
          </div>
          <div class="row">
            {#if clearPassword}
              <span class="mcp-badge mcp-badge-warn">Password will be cleared on save</span>
              <button class="btn-secondary" style="font-size:12px;" on:click={() => (clearPassword = false)}>Undo</button>
            {:else if policy.mcp_password_set}
              <span class="mcp-badge mcp-badge-ok">Password stored</span>
              <button class="btn-secondary" style="font-size:12px;" on:click={() => { clearPassword = true; accountPassword = ''; }}>Clear password</button>
            {:else}
              <span class="mcp-badge">No password stored</span>
            {/if}
          </div>
        </div>
      </fieldset>

      <!-- (h) Preview -->
      {#if preview}
        <div class="card stack">
          <div class="section-title">Test as MCP — what an agent sees</div>
          {#if preview.data_tools_disabled}
            <div class="mcp-notice mcp-notice-danger">
              <Icon name="alert-triangle" size={15} />
              <span>The global kill switch is on: all data tools (<code>query_table</code>, <code>run_query</code>) are disabled regardless of this policy. Change it in Settings → MCP server.</span>
            </div>
          {/if}
          <div class="tool-list">
            {#each preview.tools as t (t.tool)}
              <div class="tool-row">
                <code class="tool-name">{t.tool}</code>
                <span class="mcp-badge" class:mcp-badge-ok={t.allowed} class:mcp-badge-danger={!t.allowed}>{t.allowed ? 'allowed' : 'denied'}</span>
                <span class="hint">{t.reason}</span>
              </div>
            {/each}
          </div>
          <div class="hint">
            {preview.hidden_objects} object(s) hidden from agents; {preview.objects.length} visible.
          </div>
          {#if preview.objects_error}
            <div class="error-box">Could not list objects: {preview.objects_error}</div>
          {:else}
            <div class="preview-table">
              <VirtualTable columns={previewCols} rows={preview.objects} getCellClass={previewCellClass} filterable />
            </div>
          {/if}
        </div>
      {/if}
    </div>

    <div class="modal-footer" style="justify-content:space-between;">
      <button class="btn-secondary" on:click={() => void onTest()} disabled={$busy} title="Shows what an agent can do under the saved policy">
        <Icon name="play" /> Test as MCP
      </button>
      <div class="row">
        {#if dirty}<span class="hint">Unsaved changes</span>{/if}
        <button class="btn-secondary" on:click={onClosing}>Close</button>
        <button class="btn-primary" on:click={() => void persist()} disabled={$busy || !dirty}>
          <Icon name="check" /> Save
        </button>
      </div>
    </div>
  {:else}
    <div class="empty-state">Loading…</div>
  {/if}
</Modal>

<style>
  .policy-body {
    border: none;
    padding: 0;
    margin: 0;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .policy-body:disabled {
    opacity: 0.5;
  }
  .check-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--text-primary);
  }
  .check-row-lg {
    font-size: 15px;
    font-weight: 600;
  }
  .rule-row {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .rule-table {
    flex: 2;
    min-width: 0;
  }
  .rule-column {
    flex: 1;
    min-width: 0;
  }
  .field-row {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
  }
  .field-row .field {
    flex: 1;
    min-width: 200px;
  }
  .hint-inline {
    font-weight: 400;
    color: var(--text-muted);
    font-size: 11px;
    text-transform: none;
    letter-spacing: 0;
  }
  .picker {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .picker-list {
    max-height: 180px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-base);
  }
  .picker-item {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 100%;
    text-align: left;
    background: none;
    border: none;
    border-radius: 0;
    padding: 3px 8px;
    font-size: 13px;
    color: var(--text-primary);
  }
  .picker-item:hover:not(:disabled) {
    background: var(--nav-btn-hover-bg);
  }
  .picker-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .picker-type {
    font-size: 11px;
    color: var(--text-muted);
  }
  .pw-wrap {
    position: relative;
    display: flex;
  }
  .pw-wrap input {
    flex: 1;
    padding-right: 52px;
  }
  .pw-toggle {
    position: absolute;
    right: 4px;
    top: 50%;
    transform: translateY(-50%);
    background: none;
    border: none;
    padding: 2px 6px;
    font-size: 11px;
    color: var(--text-muted);
  }
  .tool-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .tool-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .tool-name {
    min-width: 170px;
    font-family: 'Consolas', monospace;
    font-size: 12px;
    color: var(--text-accent);
  }
  .preview-table {
    display: flex;
    flex-direction: column;
    height: 280px;
  }
</style>
