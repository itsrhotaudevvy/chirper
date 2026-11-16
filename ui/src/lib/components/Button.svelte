<script lang="ts">
  import type { Snippet } from 'svelte';
  import { bannerCss, NETWORK_SLOTS } from '../spectrum';

  interface Props {
    /** primary: filled with the context colour. quiet: text on a subtle surface. */
    variant?: 'primary' | 'quiet';
    /** Context colour: a network id (its bright banner colour, dark ink), or a CSS colour (ink on-accent). Defaults to the accent. */
    color?: string;
    type?: 'button' | 'submit';
    disabled?: boolean;
    onclick?: (event: MouseEvent) => void;
    children: Snippet;
  }

  let { variant = 'primary', color, type = 'button', disabled = false, onclick, children }: Props = $props();

  /* Network and banner colours are bright, so their label is the banner ink; other colours take on-accent. */
  const isNetwork = $derived(color !== undefined && color in NETWORK_SLOTS);
  const ctx = $derived(color ? (isNetwork ? bannerCss(color) : color) : 'var(--accent)');
  const ink = $derived(isNetwork || color?.startsWith('var(--banner-') ? 'var(--on-banner)' : 'var(--on-accent)');
</script>

<button class="btn {variant}" {type} {disabled} {onclick} style:--ctx={ctx} style:--ink={ink}>
  {@render children()}
</button>

<style>
  .btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--s-2);
    min-height: var(--hit);
    padding: 0 var(--s-5);
    border-radius: var(--r-pill);
    font-weight: 600;
    transition:
      transform var(--dur-quick) var(--ease-out),
      background-color var(--dur-base) var(--ease-out);
  }

  .btn:active:not(:disabled) {
    transform: scale(0.98);
  }

  .btn:disabled {
    opacity: 0.45;
  }

  .btn:focus-visible {
    outline-offset: 3px;
  }

  .primary {
    background: var(--ctx);
    color: var(--ink);
  }

  .quiet {
    background: var(--surface-high);
    color: var(--on);
  }

  @media (hover: hover) and (pointer: fine) {
    .quiet:hover:not(:disabled) {
      background: var(--outline);
    }
  }
</style>
