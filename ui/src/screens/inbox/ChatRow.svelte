<script lang="ts">
  import type { Chat } from '../../lib/core';
  import Avatar from '../../lib/components/Avatar.svelte';
  import Icon from '../../lib/components/Icon.svelte';
  import TypingDots from '../../lib/components/TypingDots.svelte';
  import UnreadPill from '../../lib/components/UnreadPill.svelte';
  import { haptic } from '../../lib/haptics';
  import { networkCss } from '../../lib/spectrum';
  import { formatRowTime, previewText, type ChatAction } from './model';

  interface Props {
    chat: Chat;
    /** Current time, passed down so every row formats times the same way. */
    now: number;
    /** Split view: the chat open in the detail pane (2 px outline, not a fill). */
    selected?: boolean;
    /** Keyboard cursor (↑/↓): a quiet surface behind the row. */
    cursor?: boolean;
    onopen: (chat: Chat) => void;
    onmenu: (chat: Chat, x: number, y: number) => void;
    onswipe: (chat: Chat, action: ChatAction) => void;
  }

  let { chat, now, selected = false, cursor = false, onopen, onmenu, onswipe }: Props = $props();

  const SWIPE_AT = 88;
  const SLOP = 8;
  const LONG_PRESS_MS = 500;

  const color = $derived(networkCss(chat.network));
  const time = $derived(formatRowTime(chat.lastActivity, now));
  const muted = $derived(chat.mutedUntil > now);
  const rightAction = $derived<ChatAction>(chat.unreadCount > 0 ? 'read' : 'pin');
  const rightLabel = $derived(rightAction === 'read' ? 'Read' : chat.pinned ? 'Unpin' : 'Pin');
  const archiveLabel = $derived(chat.archived ? 'Unarchive' : 'Archive');
  const preview = $derived(previewText(chat));
  const label = $derived(
    [chat.name, chat.unreadCount > 0 ? `${chat.unreadCount} unread` : '', time].filter(Boolean).join(', '),
  );

  let dx = $state(0);
  let dragging = $state(false);
  /** Set when a gesture (swipe, long press, context menu) has already handled the press. */
  let suppressClick = false;
  let press: { x: number; y: number; touch: boolean; swiping: boolean } | null = null;
  let pressTimer: ReturnType<typeof setTimeout> | undefined;

  function clearPress(): void {
    if (pressTimer !== undefined) clearTimeout(pressTimer);
    pressTimer = undefined;
  }

  function reset(): void {
    clearPress();
    press = null;
    dragging = false;
    dx = 0;
  }

  function onpointerdown(event: PointerEvent): void {
    if (event.button !== 0) return;
    press = { x: event.clientX, y: event.clientY, touch: event.pointerType === 'touch', swiping: false };
    suppressClick = false;
    const { clientX: x, clientY: y } = event;
    pressTimer = setTimeout(() => {
      if (!press || press.swiping) return;
      suppressClick = true;
      press = null;
      haptic('medium');
      onmenu(chat, x, y);
    }, LONG_PRESS_MS);
  }

  function onpointermove(event: PointerEvent): void {
    if (!press) return;
    const ddx = event.clientX - press.x;
    const ddy = event.clientY - press.y;
    if (!press.swiping && Math.hypot(ddx, ddy) > SLOP) {
      clearPress();
      // Only touch swipes; a horizontal drag with the mouse selects text and never swipes.
      if (!press.touch || Math.abs(ddy) > Math.abs(ddx)) {
        suppressClick = true;
        press = null;
        return;
      }
      press.swiping = true;
      dragging = true;
      (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    }
    if (press?.swiping) {
      suppressClick = true;
      dx = Math.max(-SWIPE_AT * 1.6, Math.min(SWIPE_AT * 1.6, ddx));
    }
  }

  /** Swipe right pins (or marks read when there is unread), swipe left archives. */
  function onpointerup(): void {
    if (!press) return;
    const { swiping } = press;
    clearPress();
    if (swiping) {
      if (dx <= -SWIPE_AT) onswipe(chat, 'archive');
      else if (dx >= SWIPE_AT) onswipe(chat, rightAction);
    }
    reset();
  }

  function onclick(): void {
    if (suppressClick) {
      suppressClick = false;
      return;
    }
    onopen(chat);
  }

  function oncontextmenu(event: MouseEvent): void {
    event.preventDefault();
    clearPress();
    suppressClick = true;
    haptic('medium');
    onmenu(chat, event.clientX, event.clientY);
  }

  /** Keyboard users open the same menu with the Menu key or Shift+F10. */
  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== 'ContextMenu' && !(event.key === 'F10' && event.shiftKey)) return;
    event.preventDefault();
    const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
    onmenu(chat, rect.left + rect.width / 2, rect.top + rect.height / 2);
  }
</script>

<div
  class="row"
  class:selected
  class:cursor
  data-room={chat.roomId}
  style:--net={color}
  style:--pl={dx > 0 ? Math.min(1, dx / SWIPE_AT) : 0}
  style:--pr={dx < 0 ? Math.min(1, -dx / SWIPE_AT) : 0}
>
  <span class="reveal left" class:read={rightAction === 'read'} aria-hidden="true">
    <Icon name={rightAction === 'read' ? 'check-double' : 'pin'} size={20} />{rightLabel}
  </span>
  <span class="reveal right" aria-hidden="true">
    <Icon name="archive" size={20} />{archiveLabel}
  </span>
  <button
    class="face"
    class:dragging
    type="button"
    aria-label={label}
    aria-current={selected ? 'true' : undefined}
    style:transform="translateX({dx}px)"
    {onclick}
    {onkeydown}
    {oncontextmenu}
    onpointerdown={onpointerdown}
    onpointermove={onpointermove}
    onpointerup={onpointerup}
    onpointercancel={reset}
  >
    <Avatar name={chat.name} id={chat.roomId} src={chat.avatarUrl} network={chat.network} size={48} />
    <span class="text">
      <span class="line">
        {#if chat.pinned}<span class="pinned" aria-hidden="true"><Icon name="pin" size={20} /></span>{/if}
        <span class="name t-headline truncate">{chat.name}</span>
        <span class="time t-label" aria-hidden="true">{time}</span>
      </span>
      <span class="line">
        {#if chat.typing.length > 0}
          <TypingDots color={color} label="{chat.name} is typing" />
        {:else}
          <span class="preview t-body truncate">{preview}</span>
        {/if}
        {#if muted}<span class="muted" aria-hidden="true"><Icon name="mute" size={20} /></span>{/if}
        <UnreadPill count={chat.unreadCount} network={chat.network} />
      </span>
    </span>
  </button>
</div>

<style>
  .row {
    position: relative;
    height: var(--row-h);
    overflow: hidden;
    padding: 0 6px;
  }

  /* Revealed behind the face while swiping: left for pin or read, right for archive. */
  .reveal {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    gap: var(--s-2);
    padding: 0 var(--s-5);
    color: var(--on-banner);
    font-size: var(--t-label-size);
    font-weight: 700;
    pointer-events: none;
  }

  .reveal.left {
    background: var(--banner-warn);
    justify-content: flex-start;
    opacity: var(--pl, 0);
  }

  .reveal.left.read {
    background: var(--banner-spectrum-9);
  }

  .reveal.right {
    background: var(--banner-neutral);
    justify-content: flex-end;
    opacity: var(--pr, 0);
  }

  /* The avatar's left edge sits on the page gutter (the row's own 6 px inset is subtracted). */
  .face {
    position: relative;
    z-index: 1;
    display: grid;
    grid-template-columns: 52px minmax(0, 1fr);
    align-items: center;
    column-gap: var(--s-3);
    width: 100%;
    height: 100%;
    padding: 0 calc(var(--gutter) - 6px) 0 calc(var(--gutter) - 6px);
    border-radius: var(--r-md);
    text-align: start;
    color: var(--on);
    touch-action: pan-y;
    transition: transform var(--dur-base) var(--ease-out), background-color var(--dur-quick) var(--ease-out);
  }

  .face.dragging {
    transition: none;
  }

  .row.cursor .face {
    background: var(--surface-high);
  }

  .face:active:not(.dragging) {
    transform: scale(0.985);
    background: var(--surface-high);
  }

  @media (hover: hover) and (pointer: fine) {
    .row:hover .face {
      background: var(--surface-high);
    }
  }

  /* Split view selection: a 2 px outline in the network colour, never a fill. */
  .row.selected .face {
    box-shadow: inset 0 0 0 2px var(--net);
  }

  .text {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 2px;
    min-width: 0;
  }

  .line {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    min-width: 0;
    height: 22px;
  }

  .name {
    flex: 1;
    min-width: 0;
    font-weight: 650;
  }

  .time {
    flex: none;
    color: var(--on-muted);
  }

  .pinned {
    display: inline-grid;
    flex: none;
    color: var(--warn);
  }

  .preview {
    flex: 1;
    min-width: 0;
    color: var(--on-muted);
  }

  .muted {
    flex: none;
    display: inline-grid;
    color: var(--on-muted);
  }
</style>
