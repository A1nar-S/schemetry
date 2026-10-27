<script lang="ts">
  export let width = '500px';
  export let onClose: (() => void) | undefined = undefined;
</script>

<!-- svelte-ignore a11y_interactive_supports_focus -->
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
