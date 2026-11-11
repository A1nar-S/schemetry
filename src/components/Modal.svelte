<script lang="ts">
  import { onMount } from 'svelte';

  export let width = '500px';
  export let onClose: (() => void) | undefined = undefined;

  let overlay: HTMLDivElement;

  // Take focus on open so Escape works straight away (unless something inside already
  // took it, e.g. an autofocused input), and give it back to the opener on close.
  onMount(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    if (!overlay.contains(document.activeElement)) overlay.focus({ preventScroll: true });
    return () => {
      if (opener?.isConnected) opener.focus({ preventScroll: true });
    };
  });

  function onKeydown(e: KeyboardEvent) {
    // Leave Escape to a control inside that used it (e.g. closing an editor popup).
    if (e.key !== 'Escape' || e.defaultPrevented) return;
    // Only the innermost modal closes.
    e.stopPropagation();
    onClose?.();
  }
</script>

<!-- Clicking the backdrop does not close the modal (a drag that ends outside the card
     used to close it and lose edits); Escape, the X and Close buttons do. -->
<div
  bind:this={overlay}
  class="modal-overlay"
  role="dialog"
  aria-modal="true"
  tabindex="-1"
  on:keydown={onKeydown}
>
  <div class="modal-card" style="width:{width};max-width:96vw;">
    <slot />
  </div>
</div>

<style>
  .modal-overlay:focus {
    outline: none;
  }
</style>
