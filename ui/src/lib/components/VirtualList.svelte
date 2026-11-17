<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';
  import { tick, untrack } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';

  interface Props {
    items: readonly T[];
    /** Stable key per item; rows keep their DOM and measured height across updates. */
    keyOf: (item: T) => string;
    /** Renders one row. */
    row: Snippet<[T, number]>;
    /** Required: accessible name of the list. */
    label: string;
    /** Height in px used for rows not measured yet. */
    estimate?: number;
    /** Chat mode: starts and stays at the bottom while the reader is there, anchors on prepends. */
    reverse?: boolean;
    /** Extra rows rendered above and below the viewport. */
    overscan?: number;
    /** Called when the reader nears the top (ask for older items here). */
    onStart?: () => void;
  }

  let { items, keyOf, row, label, estimate = 68, reverse = false, overscan = 6, onStart }: Props = $props();

  let viewport = $state<HTMLDivElement | undefined>(undefined);
  let scrollTop = $state(0);
  let viewHeight = $state(0);
  /** Reverse lists only: whether the reader is at the bottom (so new rows keep them there). */
  let pinned = $state(true);
  const measured = new SvelteMap<string, number>();

  const resizeObserver = new ResizeObserver((entries) => {
    for (const entry of entries) {
      const el = entry.target as HTMLElement;
      const key = el.dataset['key'];
      if (key === undefined) continue;
      const h = el.getBoundingClientRect().height;
      if (measured.get(key) !== h) measured.set(key, h);
    }
  });

  $effect(() => () => resizeObserver.disconnect());

  /** Svelte action: observes a row and records its key. */
  function observe(node: HTMLElement, key: string): { update: (k: string) => void; destroy: () => void } {
    node.dataset['key'] = key;
    resizeObserver.observe(node);
    return {
      update(next: string) {
        node.dataset['key'] = next;
      },
      destroy() {
        resizeObserver.unobserve(node);
      },
    };
  }

  /** offsets[i] is the top of item i; offsets[n] is the total height. */
  const offsets = $derived.by(() => {
    const out: number[] = [0];
    let acc = 0;
    for (const item of items) {
      acc += measured.get(keyOf(item)) ?? estimate;
      out.push(acc);
    }
    return out;
  });
  const total = $derived(offsets[items.length] ?? 0);

  /** Index range to render: the rows that intersect the viewport plus overscan. */
  const range = $derived.by(() => {
    const n = items.length;
    const top = scrollTop;
    const bottom = scrollTop + viewHeight;
    let lo = 0;
    let hi = n;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if ((offsets[mid + 1] ?? 0) > top) hi = mid;
      else lo = mid + 1;
    }
    let end = lo;
    while (end < n && (offsets[end] ?? 0) < bottom) end++;
    return { first: Math.max(0, lo - overscan), last: Math.min(n, end + overscan) };
  });

  const visible = $derived(items.slice(range.first, range.last));
  const topSpace = $derived(offsets[range.first] ?? 0);
  const bottomSpace = $derived(Math.max(0, total - (offsets[range.last] ?? total)));

  function onScroll(): void {
    if (!viewport) return;
    scrollTop = viewport.scrollTop;
    pinned = total - (viewport.scrollTop + viewport.clientHeight) < 48;
    if (onStart && viewport.scrollTop < 240) onStart();
  }

  function scrollToBottom(): void {
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }

  /**
   * Scrolls so that the item with `key` is centred in the viewport. Returns false when no such item
   * is loaded. The row renders on the next frame, from the scroll position written here.
   */
  export function scrollToKey(key: string): boolean {
    const index = items.findIndex((item) => keyOf(item) === key);
    if (index < 0 || !viewport) return false;
    const top = offsets[index] ?? 0;
    const height = measured.get(key) ?? estimate;
    viewport.scrollTop = Math.max(0, top - (viewHeight - height) / 2);
    scrollTop = viewport.scrollTop;
    return true;
  }

  /** The first row that is at least partly visible, read before the list changes. */
  function anchorRow(): HTMLElement | null {
    if (!viewport) return null;
    const top = viewport.getBoundingClientRect().top;
    for (const el of viewport.querySelectorAll<HTMLElement>('[data-key]')) {
      if (el.getBoundingClientRect().bottom > top) return el;
    }
    return null;
  }

  // Before the DOM updates: remember the anchor row so that prepended items do not move the reader.
  let anchor: { key: string; el: HTMLElement; offset: number } | null = null;
  $effect.pre(() => {
    void items;
    untrack(() => {
      const el = anchorRow();
      anchor = el && viewport && el.dataset['key']
        ? { key: el.dataset['key'], el, offset: el.getBoundingClientRect().top - viewport.getBoundingClientRect().top }
        : null;
    });
  });

  // After the DOM updates: stick to the bottom in reverse mode, or restore the anchor position.
  $effect(() => {
    void total;
    void items;
    const el = viewport;
    if (!el) return;
    const previous = anchor;
    anchor = null;
    tick().then(() => {
      if (reverse && pinned) {
        scrollToBottom();
      } else if (previous && previous.el.isConnected) {
        const now = previous.el.getBoundingClientRect().top - el.getBoundingClientRect().top;
        el.scrollTop += now - previous.offset;
      }
      scrollTop = el.scrollTop;
    });
  });
</script>

<div class="viewport" bind:this={viewport} bind:clientHeight={viewHeight} onscroll={onScroll}>
  <div class="spacer" style:height="{topSpace}px" aria-hidden="true"></div>
  <div class="rows" role="list" aria-label={label}>
    {#each visible as item, i (keyOf(item))}
      <div class="row" role="listitem" use:observe={keyOf(item)}>
        {@render row(item, range.first + i)}
      </div>
    {/each}
  </div>
  <div class="spacer" style:height="{bottomSpace}px" aria-hidden="true"></div>
</div>

<style>
  .viewport {
    position: relative;
    height: 100%;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    -webkit-overflow-scrolling: touch;
  }

  .spacer {
    width: 100%;
  }

  .row {
    contain: layout style;
  }
</style>
