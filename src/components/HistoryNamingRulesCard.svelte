<script lang="ts">
  import { onMount } from 'svelte';
  import type { HistoryNamingRule } from '../types';
  import { listHistoryNamingRules, saveHistoryNamingRule, deleteHistoryNamingRule } from '../api';
  import { busy, setBusy, notify } from '../stores/notification';

  let rules: HistoryNamingRule[] = [];
  let loaded = false;

  onMount(async () => {
    await reload();
  });

  async function reload() {
    try {
      rules = await listHistoryNamingRules();
    } catch (e) {
      notify(`Failed to load history naming rules: ${String(e)}`, 'error');
    } finally {
      loaded = true;
    }
  }

  async function persist(rule: HistoryNamingRule) {
    setBusy(true, 'Saving naming rule…');
    try {
      const saved = await saveHistoryNamingRule(rule);
      rules = rules.map(r => (r === rule ? saved : r));
    } catch (e) {
      notify(`Failed to save naming rule: ${String(e)}`, 'error');
      await reload();
    } finally {
      setBusy(false);
    }
  }

  function addRule() {
    const rule: HistoryNamingRule = { id: 0, match_type: 'prefix', pattern: '', enabled: true };
    rules = [...rules, rule];
  }

  function onFieldChange(rule: HistoryNamingRule) {
    if (!rule.pattern.trim()) return;
    void persist(rule);
  }

  async function onDelete(rule: HistoryNamingRule) {
    if (rule.id === 0) {
      rules = rules.filter(r => r !== rule);
      return;
    }
    setBusy(true, 'Deleting naming rule…');
    try {
      await deleteHistoryNamingRule(rule.id);
      rules = rules.filter(r => r.id !== rule.id);
    } catch (e) {
      notify(`Failed to delete naming rule: ${String(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  }
</script>

<div class="card stack">
  <p class="hint">
    Rules that pair a main table with its history-table counterpart, e.g. a
    <strong>Starts with</strong> rule for <code>HIST_</code> matches <code>HIST_ORDERS</code>
    to <code>ORDERS</code>. Multiple rules can be enabled at once so several naming
    conventions can coexist. Changes take effect on the next fix generation.
  </p>

  {#if loaded && rules.length === 0}
    <div class="hint">No naming rules yet — no history tables will be paired.</div>
  {/if}

  {#each rules as rule (rule)}
    <div class="rule-row">
      <select bind:value={rule.match_type} on:change={() => onFieldChange(rule)}>
        <option value="prefix">Starts with</option>
        <option value="suffix">Ends with</option>
      </select>
      <input
        class="rule-pattern"
        bind:value={rule.pattern}
        placeholder="e.g. HIST_"
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
