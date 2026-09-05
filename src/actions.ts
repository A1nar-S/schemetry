/** Svelte action that correctly sets the `indeterminate` DOM *property*
 *  (as opposed to the HTML attribute, which browsers ignore). */
export function indeterminate(node: HTMLInputElement, value: boolean) {
  node.indeterminate = value;
  return {
    update(v: boolean) {
      node.indeterminate = v;
    },
  };
}
