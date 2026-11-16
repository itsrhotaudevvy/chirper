<script lang="ts">
  import type { Snippet } from 'svelte';
  import { untrack } from 'svelte';
  import { blendForNetworks } from '../spectrum';

  interface Props {
    /** Network ids. One is a solid colour; several blend along the rainbow (blendForNetworks). */
    networks?: string[];
    /** Raw CSS colours. Several are blended into one gradient, for contexts that are not networks. */
    colors?: string[];
    /** One bold line (display type). */
    title: string;
    /** Optional single action, shown as a light pill on the colour. */
    action?: { label: string; onclick: () => void };
    /** Optional status word under the title, for example "3 unread". */
    status?: string;
    /** Trailing controls (icon buttons), on the title row. */
    trailing?: Snippet;
    /** Extra content under the title (kept short). */
    children?: Snippet;
  }

  let { networks, colors, title, action, status, trailing, children }: Props = $props();

  /*
   * DESIGN 3.3 v2: banners are bright in every theme (the bright column, never the page theme),
   * so the fill is computed without the theme. Text sits directly on the colour.
   */
  const fill = $derived.by(() => {
    if (networks && networks.length > 0) return blendForNetworks(networks);
    if (colors && colors.length > 1) return `linear-gradient(90deg, ${colors.join(', ')})`;
    return colors?.[0] ?? 'var(--banner-neutral)';
  });

  /*
   * Colour change: the new fill fades in over the previous one, which stays until the fade ends
   * (DESIGN 5: the banner colour cross-fades when the network changes). CSS animation only.
   */
  let previous = $state<string | null>(null);
  let current = untrack(() => fill);
  $effect.pre(() => {
    const next = fill;
    untrack(() => {
      if (next !== current) {
        previous = current;
        current = next;
      }
    });
  });
</script>

<header class="banner" data-tauri-drag-region>
  {#if previous}
    <div class="fill" aria-hidden="true" style:background={previous}></div>
  {/if}
  {#key fill}
    <div class="fill fresh" aria-hidden="true" style:background={fill} onanimationend={() => (previous = null)}></div>
  {/key}
  <div class="row">
    <div class="text">
      <h1 class="title t-display">{title}</h1>
      {#if status}<p class="status t-label" aria-live="polite">{status}</p>{/if}
      {@render children?.()}
    </div>
    {#if trailing || action}
      <div class="trailing">
        {@render trailing?.()}
        {#if action}<button class="on-banner-control action t-headline" type="button" onclick={action.onclick}>{action.label}</button>{/if}
      </div>
    {/if}
  </div>
</header>

<style>
  /*
   * DESIGN 3.3 v2: a solid bright block with a crisp bottom edge (radius --banner-radius), one soft
   * highlight from the top-right, and no veil, fade or dark band.
   */
  .banner {
    position: relative;
    isolation: isolate;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    min-height: calc(var(--banner-h) + var(--safe-top));
    padding: calc(var(--safe-top) + var(--titlebar-inset) + var(--s-4)) calc(var(--gutter) + var(--safe-right)) var(--s-5)
      calc(var(--gutter) + var(--safe-left));
    border-radius: 0 0 var(--banner-radius) var(--banner-radius);
    overflow: hidden;
    color: var(--on-banner);
    user-select: none;
    -webkit-user-select: none;
  }

  .fill {
    position: absolute;
    inset: 0;
    z-index: -2;
  }

  .fill.fresh {
    animation: banner-in var(--dur-base) var(--ease-out) both;
  }

  @keyframes banner-in {
    from {
      opacity: 0;
    }
  }

  .banner::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    background: radial-gradient(80% 120% at 85% 0%, rgb(255 255 255 / 0.22), transparent 70%);
  }

  .row {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--s-3);
    min-width: 0;
  }

  .text {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-1);
    min-width: 0;
    flex: 1;
  }

  .title {
    margin: 0;
    font-weight: 700;
    overflow-wrap: anywhere;
    color: var(--on-banner);
  }

  .status {
    font-weight: 650;
  }

  .trailing {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    flex: none;
  }

  .action {
    min-height: var(--hit);
    padding: 0 var(--s-4);
    border-radius: var(--r-pill);
    font-weight: 650;
  }

  @media (min-width: 900px) {
    .banner {
      min-height: var(--banner-h-desktop);
      padding-top: calc(var(--s-4) + var(--titlebar-inset));
      padding-bottom: var(--s-4);
      padding-left: var(--banner-start-desktop, calc(var(--gutter) + var(--safe-left)));
    }
  }
</style>
