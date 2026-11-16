<script lang="ts">
  import type { Snippet } from 'svelte';
  import Icon from '../../lib/components/Icon.svelte';

  interface Props {
    id: string;
    /** CSS colour of the section banner (a settings category token). */
    color: string;
    title: string;
    /** One short line on the banner, for example "3 accounts". */
    summary?: string;
    open: boolean;
    ontoggle: () => void;
    children: Snippet;
  }

  let { id, color, title, summary = '', open, ontoggle, children }: Props = $props();

  const panelId = $derived(`settings-${id}`);
</script>

<section class="section" style:--c={color}>
  <button class="banner" type="button" aria-expanded={open} aria-controls={panelId} onclick={ontoggle}>
    <span class="text">
      <span class="title t-title">{title}</span>
      {#if summary}<span class="summary t-label">{summary}</span>{/if}
    </span>
    <span class="chevron" class:open aria-hidden="true"><Icon name="chevron-right" size={24} /></span>
  </button>
  <div id={panelId} class="panel" class:open inert={!open}>
    <div class="inner">
      <div class="rows">
        {@render children()}
      </div>
    </div>
  </div>
</section>

<style>
  /* A category card: a solid bright banner (DESIGN 3.3 v2) over the rows of its section. */
  .section {
    border-radius: var(--r-xl);
    overflow: hidden;
    background: var(--surface);
  }

  .banner {
    position: relative;
    isolation: isolate;
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    gap: var(--s-3);
    width: 100%;
    min-height: 96px;
    padding: var(--s-4) var(--gutter);
    color: var(--on-banner);
    background: var(--c);
    text-align: start;
  }

  .banner::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    background: radial-gradient(90% 140% at 100% 0%, rgb(255 255 255 / 0.22), transparent 70%);
  }

  .text {
    display: grid;
    gap: var(--s-1);
    min-width: 0;
    justify-items: start;
  }

  .title {
    font-weight: 700;
  }

  .summary {
    font-weight: 650;
  }

  .chevron {
    display: grid;
    place-items: center;
    flex: none;
    width: var(--hit);
    height: var(--hit);
    border-radius: var(--r-pill);
    background: rgb(255 255 255 / 0.55);
    color: var(--on-banner);
    transition: transform var(--dur-base) var(--ease-out);
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  /* Height animates with a grid track, so the content needs no measuring. Closed rows are inert. */
  .panel {
    display: grid;
    grid-template-rows: 0fr;
    transition: grid-template-rows var(--dur-page) var(--ease-out);
  }

  .panel.open {
    grid-template-rows: 1fr;
  }

  .inner {
    min-height: 0;
    overflow: hidden;
  }

  .rows {
    display: grid;
    gap: var(--s-1);
    padding: var(--s-2) 0 var(--s-3);
  }
</style>
