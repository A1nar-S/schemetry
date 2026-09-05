<script lang="ts">
  import { onMount } from 'svelte';
  import type { TableFilterRule } from '../types';
  import { listTableFilterRules, saveTableFilterRule, deleteTableFilterRule } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';

  let rules: TableFilterRule[] = [];
  let loaded = false;

  onMount(async () => {
    await reload();
  });

  async function reload() {
    try {
      rules = await listTableFilterRules();
    } catch (e) {
      notify(`Failed to load table filter rules: ${String(e)}`, 'error');
    } finally {
      loaded = true;
    }
  }

  async function persist(rule: TableFilterRule) {
    setBusy(true, 'Saving filter rule…');
    try {
      const saved = await saveTableFilterRule(rule);
      rules = rules.map(r => (r === rule ? saved : r));
    } catch (e) {
      notify(`Failed to save filter rule: ${String(e)}`, 'error');
      await reload();
    } finally {
      setBusy(false);
    }
  }

  function addRule() {
    const rule: TableFilterRule = { id: 0, action: 'exclude', match_type: 'prefix', pattern: '', enabled: true };
    rules = [...rules, rule];
  }

  function onFieldChange(rule: TableFilterRule) {
    if (!rule.pattern.trim()) return;
    void persist(rule);
  }

  async function onDelete(rule: TableFilterRule) {
    if (rule.id === 0) {
      rules = rules.filter(r => r !== rule);
      return;
    }
    setBusy(true, 'Deleting filter rule…');
    try {
      await deleteTableFilterRule(rule.id);
      rules = rules.filter(r => r.id !== rule.id);
    } catch (e) {
      notify(`Failed to delete filter rule: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="card stack">
  <div class="section-title">Table Name Filters</div>
  <p class="hint">
    Rules that control which tables (and, for the DDL browser, other schema objects) are
    included when fetching from Oracle. <strong>Exclude</strong> rules hide matching names;
    if any <strong>Include only</strong> rules are enabled, a name must match at least one of
    them to appear at all. Changes take effect on the next fetch.
  </p>

  {#if loaded && rules.length === 0}
    <div class="hint">No filter rules yet — everything is included.</div>
  {/if}

  {#each rules as rule (rule)}
    <div class="rule-row">
      <select bind:value={rule.action} on:change={() => onFieldChange(rule)}>
        <option value="exclude">Exclude</option>
        <option value="include">Include only</option>
      </select>
      <select bind:value={rule.match_type} on:change={() => onFieldChange(rule)}>
        <option value="prefix">Starts with</option>
        <option value="suffix">Ends with</option>
        <option value="contains">Contains</option>
        <option value="exact">Exact match</option>
      </select>
      <input
        class="rule-pattern"
        bind:value={rule.pattern}
        placeholder="e.g. TEST_"
        on:blur={() => onFieldChange(rule)}
      />
      <label class="check-row">
        <input type="checkbox" bind:checked={rule.enabled} on:change={() => onFieldChange(rule)} />
        On
      </label>
      <button class="btn-danger" style="padding:4px 8px;font-size:12px;" on:click={() => void onDelete(rule)} disabled={$busy}>
        🗑
      </button>
    </div>
  {/each}

  <button class="btn-secondary" style="align-self:flex-start;font-size:12px;" on:click={addRule}>+ Add Rule</button>
</div>

<style>
  .rule-row {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }
  .rule-pattern {
    flex: 1;
  }
  .check-row {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }
</style>
