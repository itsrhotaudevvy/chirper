<script lang="ts" module>
  /* Message ids that already played their arrival animation. Virtual rows re-mount when scrolled back. */
  const arrived = new Set<string>();
</script>

<script lang="ts">
  /*
   * One message in the timeline: alignment, sender group geometry, the arrival animation, and the
   * gestures. Swipe right replies; long press (touch) or right click opens the actions; double tap
   * or double click gives a heart; the hover toolbar is for desktop pointers.
   */
  import type { Message } from '../../lib/core';
  import type { MessageRow as Row } from './grouping';
  import { timeLabel } from './grouping';
  import { haptic } from '../../lib/haptics';
  import { dur } from '../../lib/motion';
  import { personHue } from '../../lib/spectrum';
  import Icon from '../../lib/components/Icon.svelte';
  import IconButton from '../../lib/components/IconButton.svelte';
  import MessageBody from './MessageBody.svelte';
  import Reactions from './Reactions.svelte';

  interface Props {
    row: Row;
    onreply: (m: Message) => void;
    onmenu: (m: Message) => void;
    ontoggle: (m: Message, key: string) => void;
    onheart: (m: Message) => void;
    onopen: (src: string | null, kind: 'image' | 'video') => void;
    onretry: (m: Message) => void;
    /** Reports whether this (newest) message is on screen. */
    onnewest: (visible: boolean) => void;
    /** Search jumped here: the row flashes once in the chat colour. */
    flash?: boolean;
  }

  let { row, onreply, onmenu, ontoggle, onheart, onopen, onretry, onnewest, flash = false }: Props = $props();

  const LONG_PRESS_MS = 450;
  const SWIPE_PX = 64;
  const SWIPE_MAX = 84;

  const m = $derived(row.message);
  const out = $derived(m.fromMe);
  const top = $derived(row.first ? 18 : 6);
  const bottom = $derived(row.last ? 18 : 6);
  const sideStyle = $derived(
    out ? `--tr: ${top}px; --br: ${bottom}px;` : `--tl: ${top}px; --bl: ${bottom}px;`,
  );
  const failed = $derived(out && m.status === 'failed');
  const showMeta = $derived(row.last || failed);

  let swipe = $state(0);
  let burst = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;
  let startX = 0;
  let startY = 0;
  let pointerType = '';
  let lastTap = 0;
  let moved = false;

  /* The arrival animation: a live message rises 16 px and fades in. Others render instantly. */
  function arrive(node: HTMLElement, live: boolean): { destroy: () => void } {
    if (live && !arrived.has(m.eventId)) {
      arrived.add(m.eventId);
      node.animate(
        [
          { transform: 'translateY(16px)', opacity: 0 },
          { transform: 'translateY(0)', opacity: 1 },
        ],
        { duration: dur(220), easing: 'cubic-bezier(0.2, 0.8, 0.2, 1)' },
      );
    }
    return { destroy() {} };
  }

  /* Reports visibility of the newest row (drives mark-read). */
  function watchNewest(node: HTMLElement, enabled: boolean): { update: (v: boolean) => void; destroy: () => void } {
    let io: IntersectionObserver | undefined;
    const set = (on: boolean): void => {
      if (io) onnewest(false);
      io?.disconnect();
      io = undefined;
      if (!on || typeof IntersectionObserver === 'undefined') return;
      io = new IntersectionObserver((entries) => onnewest(entries.some((e) => e.isIntersecting)), { threshold: 0.6 });
      io.observe(node);
    };
    set(enabled);
    return {
      update: set,
      destroy() {
        if (io) onnewest(false);
        io?.disconnect();
      },
    };
  }

  function cancelPress(): void {
    clearTimeout(timer);
    timer = undefined;
  }

  function down(event: PointerEvent): void {
    if (event.button !== 0) return;
    pointerType = event.pointerType;
    startX = event.clientX;
    startY = event.clientY;
    moved = false;
    cancelPress();
    timer = setTimeout(() => {
      timer = undefined;
      if (!moved) {
        moved = true;
        haptic('medium');
        onmenu(m);
      }
    }, LONG_PRESS_MS);
  }

  function move(event: PointerEvent): void {
    const dx = event.clientX - startX;
    const dy = event.clientY - startY;
    if (Math.abs(dx) > 8 || Math.abs(dy) > 8) {
      moved = true;
      cancelPress();
    }
    if (pointerType === 'touch' && moved && dx > 0 && Math.abs(dy) < Math.abs(dx)) {
      swipe = Math.min(SWIPE_MAX, dx);
    }
  }

  function up(): void {
    cancelPress();
    if (swipe >= SWIPE_PX) {
      haptic('selection');
      onreply(m);
    }
    swipe = 0;
    if (!moved) tapAt(Date.now());
    moved = false;
  }

  function tapAt(now: number): void {
    if (now - lastTap < 300) {
      lastTap = 0;
      heart();
    } else {
      lastTap = now;
    }
  }

  function heart(): void {
    if (!m.redacted) onheart(m);
    burst = false;
    requestAnimationFrame(() => (burst = true));
  }

  function contextMenu(event: MouseEvent): void {
    event.preventDefault();
    if (!m.redacted) onmenu(m);
  }

  function toggle(key: string): void {
    ontoggle(m, key);
  }
</script>

<div
  class="msg"
  class:out
  class:first={row.first}
  class:last={row.last}
  class:flash
  style={sideStyle}
  use:arrive={m.live}
>
  {#if row.showName}
    <span class="name t-label" style:--hue={personHue(m.sender)}>{m.senderName}</span>
  {/if}

  <div class="line">
    <div
      class="stack"
      role="presentation"
      style:transform={swipe ? `translateX(${swipe}px)` : undefined}
      class:swiping={swipe > 0}
      onpointerdown={down}
      onpointermove={move}
      onpointerup={up}
      onpointercancel={cancelPress}
      oncontextmenu={contextMenu}
      use:watchNewest={row.newest}
    >
      <div class="content" class:jumbo={row.jumbo !== null}>
        <MessageBody message={m} jumbo={row.jumbo} {onopen} />
      </div>
      {#if m.reactions.length > 0}
        <div class="reactions-wrap">
          <Reactions message={m} ontoggle={toggle} />
        </div>
      {/if}
      {#if burst}
        <span class="burst" aria-hidden="true" onanimationend={() => (burst = false)}>
          <svg viewBox="0 0 24 24"><path d="M12 20.5l-1.4-1.3C5.6 14.6 2.5 11.8 2.5 8.3 2.5 5.5 4.7 3.5 7.5 3.5c1.6 0 3.1.8 4.5 2.1 1.4-1.3 2.9-2.1 4.5-2.1 2.8 0 5 2 5 4.8 0 3.5-3.1 6.3-8.1 10.9L12 20.5z" /></svg>
        </span>
      {/if}
    </div>

    <div class="toolbar" aria-hidden="true">
      {#if !m.redacted}
        <IconButton icon="reply" label="Reply" onclick={() => onreply(m)} />
        <IconButton icon="more" label="Message actions" onclick={() => onmenu(m)} />
      {/if}
    </div>
  </div>

  {#if showMeta}
    <div class="meta t-label">
      {#if m.edited && !m.redacted}<span class="edited">edited</span>{/if}
      {#if row.last}<span>{timeLabel(m.ts)}</span>{/if}
      {#if out && !failed}
        {#if m.status === 'sending'}
          <Icon name="check" size={20} />
        {:else if m.status === 'sent'}
          <Icon name="check" size={20} />
        {:else if m.status === 'delivered'}
          <Icon name="check-double" size={20} />
        {:else if m.status === 'read'}
          <span class="read"><Icon name="check-double" size={20} /></span>
        {/if}
      {/if}
      {#if failed}
        <button class="retry" type="button" onclick={() => onretry(m)}>Retry</button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .msg {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    margin-top: 3px;
    padding: 0 var(--gutter);
    --box-bg: var(--surface-high);
    --tl: 18px;
    --tr: 18px;
    --br: 18px;
    --bl: 18px;
  }

  .msg.first {
    margin-top: 12px;
  }

  /* Search jump: the message flashes in the chat colour at 20 % (DESIGN 5), once. */
  .msg.flash {
    animation: flash 900ms var(--ease-out) both;
  }

  @keyframes flash {
    from {
      background: color-mix(in srgb, var(--ctx) 20%, transparent);
    }
  }

  .msg.out {
    align-items: flex-end;
    --box-bg: color-mix(in srgb, var(--ctx) 18%, var(--surface));
  }

  /* Names, times and ticks sit on the text's own edge: no inset, so they line up with the words. */
  .name {
    padding: 0 0 2px;
    color: oklch(var(--name-l, 0.5) 0.13 var(--hue));
    font-weight: 650;
  }

  .line {
    position: relative;
    display: flex;
    align-items: center;
    gap: var(--s-1);
    max-width: 100%;
  }

  .out .line {
    flex-direction: row-reverse;
  }

  .stack {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: var(--s-1);
    max-width: min(84%, 560px);
    min-width: 0;
    touch-action: pan-y;
    transition: transform var(--dur-base) var(--ease-out);
  }

  .stack.swiping {
    transition: none;
  }

  .out .stack {
    align-items: flex-end;
  }

  .content {
    min-width: 0;
    color: var(--on);
  }

  .out .content:not(.jumbo) :global(.text) {
    color: color-mix(in srgb, var(--ctx) 35%, var(--on));
    text-align: right;
  }

  .out .content:not(.jumbo) :global(.box .text) {
    color: var(--on);
    text-align: left;
  }

  .jumbo {
    padding: var(--s-1) 0;
  }

  .reactions-wrap {
    margin-top: -2px;
  }

  .burst {
    position: absolute;
    left: 50%;
    top: 50%;
    width: 0;
    height: 0;
    pointer-events: none;
  }

  .burst svg {
    position: absolute;
    left: -16px;
    top: -16px;
    width: 32px;
    height: 32px;
    fill: var(--danger);
    animation: heart var(--spring-bounce-dur) var(--spring-bounce) both;
  }

  @keyframes heart {
    0% {
      transform: scale(0.2);
      opacity: 0;
    }
    30% {
      transform: scale(1.2);
      opacity: 1;
    }
    100% {
      transform: scale(1) translateY(-18px);
      opacity: 0;
    }
  }

  .toolbar {
    display: none;
    gap: 2px;
    flex-direction: row;
  }

  @media (hover: hover) and (pointer: fine) {
    .toolbar {
      display: flex;
      opacity: 0;
      transition: opacity var(--dur-quick) var(--ease-out);
    }
    .line:hover .toolbar,
    .line:focus-within .toolbar {
      opacity: 1;
    }
  }

  /* Time and ticks: flush with the text's left edge (incoming) or right edge (outgoing). */
  .meta {
    display: flex;
    align-items: center;
    gap: var(--s-1);
    padding: 0;
    color: color-mix(in srgb, var(--on) 62%, transparent);
    font-variant-numeric: tabular-nums;
  }

  .read {
    color: var(--ctx);
    display: inline-flex;
  }

  .edited {
    font-style: italic;
  }

  .retry {
    min-height: var(--hit);
    padding: 0;
    color: var(--danger);
    font-weight: 700;
  }

  .msg {
    --name-l: 0.5;
  }

  @media (prefers-color-scheme: dark) {
    :root:not([data-theme='light']) .msg {
      --name-l: 0.78;
    }
  }

  :root[data-theme='dark'] .msg,
  :root[data-theme='amoled'] .msg {
    --name-l: 0.78;
  }
</style>
