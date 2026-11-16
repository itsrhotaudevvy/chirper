<script lang="ts">
  import type { Snippet } from 'svelte';
  import { untrack } from 'svelte';
  import { blendForNetworks } from '../spectrum';
  import Icon from './Icon.svelte';

  interface Props {
    /** Network ids. One is a solid colour; several blend along the rainbow (blendForNetworks). */
    networks?: string[];
    /** Raw CSS colours. Several are blended into one gradient, for contexts that are not networks. */
    colors?: string[];
    /** One bold line. */
    title: string;
    /** Optional single action, shown as a light pill on the colour or combined with title if back/cancel. */
    action?: { label: string; onclick: () => void };
    /** Optional status word under the title, for example "3 unread". */
    status?: string;
    /** Trailing controls (icon buttons), on the title row. */
    trailing?: Snippet;
    /** Extra content under the title (kept short). */
    children?: Snippet;
    /** When true, animates the title out (e.g. when search takes over the header). */
    hideTitle?: boolean;
    /** Optional explicit back handler to combine with the title. */
    onback?: () => void;
  }

  let {
    networks,
    colors,
    title,
    action,
    status,
    trailing,
    children,
    hideTitle = false,
    onback,
  }: Props = $props();

  /*
   * DESIGN 3.3 v2: banners are bright in every theme (the bright column, never the page theme),
   * so the fill is computed without the theme. Text sits directly on the colour.
   */
  const fill = $derived.by(() => {
    if (networks && networks.length > 0) return blendForNetworks(networks);
    if (colors && colors.length > 1) return `linear-gradient(90deg, ${colors.join(', ')})`;
    return colors?.[0] ?? 'var(--banner-neutral)';
  });

  const isBackAction = $derived.by(() => {
    if (onback) return true;
    if (!action) return false;
    const l = action.label.trim().toLowerCase();
    return l === 'back' || l === 'cancel' || l === 'close';
  });

  const backHandler = $derived(onback ?? (isBackAction ? action?.onclick : undefined));

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
  <div class="main-row">
    <div class="title-wrap" class:hidden-title={hideTitle}>
      {#if backHandler}
        <button class="title-action" type="button" onclick={backHandler} aria-label={action?.label ?? 'Go back'}>
          <span class="back-icon"><Icon name="back" size={24} /></span>
          <h1 class="title t-title">{title}</h1>
        </button>
      {:else}
        <h1 class="title t-title">{title}</h1>
      {/if}
    </div>
    {#if hideTitle && children}
      <div class="search-wrap">
        {@render children()}
      </div>
    {/if}
    {#if trailing || (action && !isBackAction)}
      <div class="trailing">
        {@render trailing?.()}
        {#if action && !isBackAction}
          <button class="on-banner-control action t-headline" type="button" onclick={action.onclick}>{action.label}</button>
        {/if}
      </div>
    {/if}
  </div>
  {#if !hideTitle}
    {#if status}
      <p class="status t-label" aria-live="polite">{status}</p>
    {/if}
    {@render children?.()}
  {/if}
</header>

<style>
  /*
   * Solid bright block with compact height, clean radius, and soft top-right highlight.
   */
  .banner {
    position: relative;
    isolation: isolate;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    min-height: calc(var(--banner-h) + var(--safe-top));
    padding: calc(var(--safe-top) + var(--titlebar-inset) + var(--s-2)) calc(var(--gutter) + var(--safe-right)) var(--s-3)
      calc(var(--gutter) + var(--safe-left));
    border-radius: 0 0 var(--banner-radius) var(--banner-radius);
    overflow: hidden;
    color: var(--on-banner);
    user-select: none;
    -webkit-user-select: none;
    transition: min-height var(--dur-base) var(--ease-out), padding var(--dur-base) var(--ease-out);
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

  .main-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    min-width: 0;
    min-height: var(--hit);
  }

  .title-wrap {
    display: flex;
    align-items: center;
    min-width: 0;
    flex: 1 1 auto;
    transition:
      opacity var(--dur-base) var(--ease-out),
      transform var(--dur-base) var(--ease-out);
  }

  .title-wrap.hidden-title {
    display: none;
  }

  .search-wrap {
    display: flex;
    align-items: center;
    flex: 1 1 auto;
    min-width: 0;
  }

  .title {
    margin: 0;
    font-weight: 700;
    line-height: 1.1;
    display: flex;
    align-items: center;
    overflow-wrap: anywhere;
    color: var(--on-banner);
    letter-spacing: -0.01em;
  }

  .title-action {
    display: inline-flex;
    align-items: center;
    gap: var(--s-1);
    background: transparent;
    border: 0;
    padding: 2px var(--s-2) 2px 0;
    margin: -2px 0;
    border-radius: var(--r-md);
    cursor: pointer;
    color: var(--on-banner);
    text-align: start;
    transition: transform var(--dur-quick) var(--ease-out), background var(--dur-quick) var(--ease-out);
  }

  .title-action:active {
    transform: scale(0.97);
    background: rgb(255 255 255 / 0.18);
  }

  .back-icon {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    color: var(--on-banner);
  }

  .status {
    font-weight: 650;
    margin: var(--s-1) 0 0;
  }

  .trailing {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    flex: none;
  }

  .action {
    min-height: calc(var(--hit) - 4px);
    padding: 0 var(--s-4);
    border-radius: var(--r-pill);
    font-weight: 650;
  }

  @media (min-width: 900px) {
    .banner {
      min-height: var(--banner-h-desktop);
      padding-top: calc(var(--s-2) + var(--titlebar-inset));
      padding-bottom: var(--s-3);
      padding-left: var(--banner-start-desktop, calc(var(--gutter) + var(--safe-left)));
    }
  }
</style>
