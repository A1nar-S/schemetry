<script lang="ts">
  export let width = '500px';
  export let onClose: (() => void) | undefined = undefined;
</script>

<!-- svelte-ignore a11y-click-events-have-key-events a11y-no-noninteractive-element-interactions -->
<div
  class="modal-overlay"
  role="dialog"
  aria-modal="true"
  on:click|self={() => onClose?.()}
  on:keydown={(e) => { if (e.key === 'Escape') onClose?.(); }}
>
  <div class="modal-card" style="width:{width};max-width:96vw;">
    <slot />
  </div>
</div>
