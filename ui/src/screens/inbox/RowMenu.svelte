<script lang="ts">
  import type { Chat } from '../../lib/core';
  import Icon from '../../lib/components/Icon.svelte';
  import type { IconName } from '../../lib/components/icons';
  import { trapFocus } from '../../lib/a11y';
  import { bannerCss } from '../../lib/spectrum';
  import type { ChatAction } from './model';

  interface Props {
    chat: Chat;
    /** Where the menu opens (viewport pixels: pointer, or the row for the keyboard). */
    x: number;
    y: number;
    now: number;
    onaction: (action: ChatAction) => void;
    onclose: () => void;
  }

  let { chat, x, y, now, onaction, onclose }: Props = $props();

  const WIDTH = 232;
  const HEIGHT = 300;
  const left = $derived(Math.max(8, Math.min(x, window.innerWidth - WIDTH - 8)));
  const top = $derived(Math.max(8, Math.min(y, window.innerHeight - HEIGHT - 8)));
  const muted = $derived(chat.mutedUntil > now);

  const items = $derived<Array<{ action: ChatAction; label: string; icon: IconName }>>([
    { action: 'pin', label: chat.pinned ? 'Unpin' : 'Pin to top', icon: 'pin' },
    { action: 'mute', label: muted ? 'Unmute' : 'Mute', icon: 'mute' },
    { action: 'archive', label: chat.archived ? 'Unarchive' : 'Archive', icon: 'archive' },
    { action: 'read', label: chat.unreadCount > 0 ? 'Mark as read' : 'Mark as unread', icon: 'check-double' },
  ]);

  /** Arrow keys move between items; Tab stays inside the menu (trapFocus). */
  function onkeydown(event: KeyboardEvent): void {
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    const buttons = [...(event.currentTarget as HTMLElement).querySelectorAll<HTMLButtonElement>('button')];
    const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
    const next = buttons[(at + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length];
    event.preventDefault();
    next?.focus();
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />

<div class="scrim" role="presentation" onpointerdown={onclose}></div>
<div
  class="menu"
  role="menu"
  tabindex="-1"
  aria-label="Actions for {chat.name}"
  style:left="{left}px"
  style:top="{top}px"
  style:--head={bannerCss(chat.network)}
  use:trapFocus={{ onEscape: onclose }}
  {onkeydown}
>
  <p class="head t-headline truncate">{chat.name}</p>
  {#each items as item (item.action)}
    <button class="item t-body" type="button" role="menuitem" onclick={() => onaction(item.action)}>
      <Icon name={item.icon} size={20} />
      <span>{item.label}</span>
    </button>
  {/each}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: transparent;
    transition: background var(--dur-quick) var(--ease-out);
  }

  .menu {
    position: fixed;
    z-index: 61;
    width: 232px;
    padding: 0 0 var(--s-2);
    border-radius: var(--r-lg);
    background: var(--surface);
    box-shadow: var(--elev-2);
    overflow: hidden;
    animation: menu-in var(--dur-quick) var(--ease-out) both;
  }

  /* The network banner leads: a solid strip with the chat name on it, not a list header. */
  .head {
    margin: 0 0 var(--s-2);
    padding: var(--s-3) var(--s-4);
    background: var(--head);
    color: var(--on-banner);
    font-weight: 700;
  }

  .item {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    min-height: var(--hit);
    padding: 0 var(--s-4);
    text-align: start;
    color: var(--on);
    transition: background var(--dur-quick) var(--ease-out), transform var(--dur-quick) var(--ease-out);
  }

  .item:active {
    transform: scale(0.98);
  }

  .item:focus-visible {
    background: var(--surface-high);
    outline: none;
  }

  @media (hover: hover) and (pointer: fine) {
    .item:hover {
      background: var(--surface-high);
    }
  }

  @keyframes menu-in {
    from {
      opacity: 0;
      transform: scale(0.96);
    }
  }

  @media (max-width: 640px) {
    .scrim {
      background: rgb(0 0 0 / 0.45);
      backdrop-filter: blur(2px);
      -webkit-backdrop-filter: blur(2px);
      animation: scrim-in var(--dur-base) var(--ease-out) both;
    }

    .menu {
      left: 0 !important;
      right: 0 !important;
      bottom: 0 !important;
      top: auto !important;
      width: 100% !important;
      max-width: 100% !important;
      border-radius: var(--banner-radius) var(--banner-radius) 0 0;
      padding-bottom: calc(var(--s-4) + var(--safe-bottom));
      box-shadow: var(--elev-2);
      animation: menu-slide-up var(--dur-base) var(--ease-out) both;
    }

    .head {
      margin: 0 0 var(--s-2);
      padding: var(--s-3) var(--gutter);
      font-size: 1.15rem;
      border-radius: var(--banner-radius) var(--banner-radius) 0 0;
    }

    .item {
      padding: 0 var(--gutter);
      min-height: 52px;
      font-size: 1.05rem;
      gap: var(--s-4);
    }
  }

  @keyframes scrim-in {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }

  @keyframes menu-slide-up {
    from {
      opacity: 0;
      transform: translateY(100%);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }
</style>
