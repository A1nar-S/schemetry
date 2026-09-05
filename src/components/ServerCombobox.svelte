<script lang="ts">
  import type { ConnectionRecord } from '../types';
  import { tick } from 'svelte';

  export let connections: ConnectionRecord[] = [];
  export let value = '';
  export let placeholder = 'Search a server…';
  export let disabled = false;
  export let onChange: ((value: string) => void) | undefined = undefined;

  let query = '';
  let open = false;
  let editing = false;
  let highlight = 0;
  let inputEl: HTMLInputElement;
  let listEl: HTMLDivElement;
  let wrapperEl: HTMLDivElement;

  // Mirror the selected value into the input text whenever we're not typing.
  $: if (!editing) query = value;

  // While the input still shows the current selection verbatim, treat it as an
  // empty query so the full list is browsable; the first keystroke filters.
  $: effectiveQ =
    query.trim().toLowerCase() === value.toLowerCase() ? '' : query.trim().toLowerCase();

  $: filtered = effectiveQ
    ? connections.filter(
        c =>
          c.name.toLowerCase().includes(effectiveQ) ||
          (c.group_name?.toLowerCase().includes(effectiveQ) ?? false),
      )
    : connections;

  // Grouped for display; `flat` mirrors the visible order for keyboard nav.
  $: grouped = (() => {
    const m = new Map<string, ConnectionRecord[]>();
    for (const c of filtered) {
      const k = c.group_name?.trim() || 'Default';
      if (!m.has(k)) m.set(k, []);
      m.get(k)!.push(c);
    }
    return m;
  })();
  $: flat = [...grouped.values()].flat();
  $: if (highlight >= flat.length) highlight = Math.max(0, flat.length - 1);

  function openList() {
    if (disabled) return;
    open = true;
    editing = true;
    highlight = Math.max(0, flat.findIndex(c => c.name === value));
  }

  function choose(c: ConnectionRecord) {
    value = c.name;
    query = c.name;
    editing = false;
    open = false;
    onChange?.(value);
  }

  function close() {
    open = false;
    editing = false;
    query = value;
  }

  async function scrollToHighlight() {
    await tick();
    (listEl?.querySelector('[data-hl="true"]') as HTMLElement | null)?.scrollIntoView({
      block: 'nearest',
    });
  }

  function onKeydown(e: KeyboardEvent) {
    if (!open) {
      if (e.key === 'ArrowDown' || e.key === 'Enter') {
        e.preventDefault();
        openList();
      }
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlight = Math.min(highlight + 1, flat.length - 1);
      void scrollToHighlight();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlight = Math.max(highlight - 1, 0);
      void scrollToHighlight();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (flat[highlight]) choose(flat[highlight]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      close();
    }
  }

  function onWindowPointer(e: MouseEvent) {
    if (open && wrapperEl && !wrapperEl.contains(e.target as Node)) close();
  }
</script>

<svelte:window on:mousedown={onWindowPointer} />

<div class="combobox" bind:this={wrapperEl}>
  <input
    bind:this={inputEl}
    type="text"
    autocomplete="off"
    spellcheck="false"
    {placeholder}
    {disabled}
    value={query}
    on:input={(e) => { editing = true; open = true; query = e.currentTarget.value; }}
    on:focus={() => { openList(); inputEl?.select(); }}
    on:keydown={onKeydown}
  />
  <span class="combobox-chevron" aria-hidden="true">▾</span>

  {#if open}
    <div class="combobox-list" role="listbox" bind:this={listEl}>
      {#each [...grouped.entries()] as [group, conns]}
        {#if grouped.size > 1}
          <div class="combobox-group">{group}</div>
        {/if}
        {#each conns as conn}
          {@const idx = flat.indexOf(conn)}
          <div
            class="combobox-option"
            class:combobox-option-active={conn.name === value}
            class:combobox-option-hl={idx === highlight}
            data-hl={idx === highlight}
            role="option"
            aria-selected={conn.name === value}
            tabindex="-1"
            on:mousedown|preventDefault={() => choose(conn)}
            on:mousemove={() => { highlight = idx; }}
          >
            {conn.name}
          </div>
        {/each}
      {:else}
        <div class="combobox-empty">No servers match.</div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .combobox {
    position: relative;
    width: 100%;
  }
  .combobox input {
    width: 100%;
    font-size: 13px;
    padding: 6px 26px 6px 10px;
  }
  .combobox-chevron {
    position: absolute;
    right: 9px;
    top: 50%;
    transform: translateY(-50%);
    font-size: 10px;
    color: var(--text-muted);
    pointer-events: none;
  }
  .combobox-list {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    z-index: 20;
    max-height: 280px;
    overflow-y: auto;
    background: var(--bg-surface);
    border: 1px solid var(--border-input);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    padding: 4px;
  }
  .combobox-group {
    font-size: 10px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--conn-group-color);
    padding: 6px 8px 2px;
  }
  .combobox-option {
    font-size: 12px;
    color: var(--text-primary);
    padding: 5px 8px;
    border-radius: 4px;
    cursor: pointer;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .combobox-option-hl {
    background: var(--nav-btn-hover-bg);
  }
  .combobox-option-active {
    color: var(--text-accent);
    font-weight: 600;
  }
  .combobox-empty {
    font-size: 12px;
    color: var(--text-muted);
    padding: 8px;
    text-align: center;
  }
</style>
