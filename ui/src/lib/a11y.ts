const FOCUSABLE = [
  'a[href]',
  'button:not([disabled])',
  'input:not([disabled]):not([type="hidden"])',
  'select:not([disabled])',
  'textarea:not([disabled])',
  '[tabindex]:not([tabindex="-1"])',
].join(',');

/** Visible, focusable descendants of a node, in DOM order. */
export function focusableWithin(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.getClientRects().length > 0);
}

export interface TrapOptions {
  /** Called on Escape (the dialog decides how to close). */
  onEscape?: () => void;
}

/**
 * Svelte action for dialogs: keeps Tab and Shift+Tab inside the node, closes on Escape, focuses the
 * first control on mount and restores the previously focused element on destroy.
 */
export function trapFocus(node: HTMLElement, options: TrapOptions = {}): { destroy: () => void } {
  const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const onKey = (event: KeyboardEvent): void => {
    if (event.key === 'Escape') {
      event.stopPropagation();
      options.onEscape?.();
      return;
    }
    if (event.key !== 'Tab') return;
    const items = focusableWithin(node);
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) {
      event.preventDefault();
      node.focus();
      return;
    }
    const active = document.activeElement;
    const outside = !node.contains(active);
    if (event.shiftKey && (active === first || outside)) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || outside)) {
      event.preventDefault();
      first.focus();
    }
  };
  node.addEventListener('keydown', onKey);
  (focusableWithin(node)[0] ?? node).focus();
  return {
    destroy() {
      node.removeEventListener('keydown', onKey);
      previous?.focus();
    },
  };
}

let region: HTMLElement | null = null;

/** Announces a message to screen readers through one shared, visually hidden live region. */
export function announce(message: string, politeness: 'polite' | 'assertive' = 'polite'): void {
  if (typeof document === 'undefined') return;
  if (!region) {
    region = document.createElement('div');
    region.className = 'visually-hidden';
    document.body.appendChild(region);
  }
  region.setAttribute('aria-live', politeness);
  region.setAttribute('role', politeness === 'assertive' ? 'alert' : 'status');
  region.textContent = '';
  // Setting the text after a tick makes a repeated identical message announce again.
  setTimeout(() => {
    if (region) region.textContent = message;
  }, 30);
}
