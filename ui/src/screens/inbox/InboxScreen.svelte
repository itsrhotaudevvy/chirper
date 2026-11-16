<script lang="ts">
  /*
   * Inbox (CONTRACT §D, DESIGN §0, §4, §6, §7). One colour banner: the networks of the user's
   * accounts, blended along the rainbow. Under it a network filter strip, then the chat list.
   */
  import { onMount, tick } from 'svelte';
    import type { Chat } from '../../lib/core';
  import { core } from '../../lib/core';
  import { accounts, chats, ui } from '../../lib/stores';
  import { router } from '../../lib/router.svelte';
  import { haptic } from '../../lib/haptics';
  import { announce } from '../../lib/a11y';
    import { bannerCss, networkCss } from '../../lib/spectrum';
  import Avatar from '../../lib/components/Avatar.svelte';
  import Banner from '../../lib/components/Banner.svelte';
  import Button from '../../lib/components/Button.svelte';
  import Icon from '../../lib/components/Icon.svelte';
  import IconButton from '../../lib/components/IconButton.svelte';
  import SectionBanner from '../../lib/components/SectionBanner.svelte';
  import VirtualList from '../../lib/components/VirtualList.svelte';
  import ChatRow from './ChatRow.svelte';
  import NetworkStrip from './NetworkStrip.svelte';
  import RowMenu from './RowMenu.svelte';
  import {
    MUTE_FOREVER,
    groupByNetwork,
    moveIndex,
    networksOf,
    pinnedOf,
    unreadOf,
    visibleChats,
    type ChatAction,
    type ListItem,
  } from './model';

  /** Dev-only screenshot switches, read from the hash: #/?net=telegram or #/?group=1. */
  const devParams = import.meta.env.DEV ? new URLSearchParams(location.hash.split('?')[1] ?? '') : null;

  let network = $state<string | null>(devParams?.get('net') ?? null);
  let grouped = $state(devParams?.has('group') ?? false);
  /** The archived filter: the same list, of archived chats, under a neutral banner. */
  let archivedView = $state(devParams?.has('archived') ?? false);
  let query = $state('');
  let searching = $state(false);
  let searchEl = $state<HTMLInputElement | undefined>(undefined);
  let cursorId = $state<string | null>(null);
  let menu = $state<{ chat: Chat; x: number; y: number } | null>(null);
  let notice = $state('');
  let now = $state(Date.now());
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;

  const accountNetworks = $derived([...new Set(accounts.list.map((a) => a.network))]);
  /** The chats of the current filter: active, or archived. */
  const source = $derived(archivedView ? chats.archived : chats.active);
  /** The banner: the user's connected networks in rainbow order, or one network when filtered. */
  const bannerNetworks = $derived(
    network ? [network] : accountNetworks.length > 0 ? accountNetworks : networksOf(chats.active),
  );
  const stripNetworks = $derived(networksOf(source));
  /** The "All" chip: the same rainbow as a CSS gradient (theme-aware through the tokens). */
  const allFill = $derived(
    stripNetworks.length > 1
      ? `linear-gradient(90deg, ${stripNetworks.map((n) => networkCss(n)).join(', ')})`
      : networkCss(stripNetworks[0] ?? 'demo'),
  );
  const shown = $derived(visibleChats(source, network, query));
  const unread = $derived(unreadOf(shown));

  const items = $derived.by<ListItem[]>(() => {
    if (!grouped) return shown.map((chat): ListItem => ({ key: `row:${chat.roomId}`, kind: 'row', chat }));
    return groupByNetwork(shown).flatMap(({ network: id, chats: list }) => [
      { key: `band:${id}`, kind: 'band', network: id, label: networkName(id) },
      ...list.map((chat): ListItem => ({ key: `row:${chat.roomId}`, kind: 'row', chat })),
    ]);
  });
  /** Chats in the order they are drawn, for the keyboard cursor. */
  const rowChats = $derived(items.flatMap((item) => (item.kind === 'row' ? [item.chat] : [])));

  function networkName(id: string): string {
    return accounts.network(id)?.name ?? id;
  }

  function flash(message: string): void {
    notice = message;
    announce(message, 'assertive');
    if (noticeTimer !== undefined) clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ''), 3000);
  }

  function open(chat: Chat): void {
    haptic('medium');
    router.openChat(chat.roomId);
  }

  function selectNetwork(id: string | null): void {
    if (id === network) return;
    network = id;
    haptic('selection');
  }

  function toggleGroup(): void {
    grouped = !grouped;
    haptic('selection');
  }

  function toggleArchived(): void {
    archivedView = !archivedView;
    network = null;
    haptic('selection');
  }

  async function act(chat: Chat, action: ChatAction): Promise<void> {
    const roomId = chat.roomId;
    try {
      if (action === 'pin') await core.setChatFlags({ roomId, flags: { pinned: !chat.pinned } });
      else if (action === 'mute') {
        await core.setChatFlags({ roomId, flags: { mutedUntil: chat.mutedUntil > now ? 0 : MUTE_FOREVER } });
      } else if (action === 'archive') await core.setChatFlags({ roomId, flags: { archived: !chat.archived } });
      else await core.markUnread({ roomId, unread: chat.unreadCount === 0 });
    } catch {
      flash('Could not update the chat');
    }
  }

  async function openSearch(): Promise<void> {
    searching = true;
    await tick();
    searchEl?.focus();
  }

  function closeSearch(): void {
    searching = false;
    query = '';
  }

  function onSearchKey(event: KeyboardEvent): void {
    if (event.key !== 'Escape') return;
    // Escape closes the search here, so it never reaches the app-level back handler.
    event.stopPropagation();
    closeSearch();
  }

  /** Moves the keyboard cursor and focuses that row, scrolling it into view. */
  function moveCursor(step: number): void {
    const at = rowChats.findIndex((c) => c.roomId === cursorId);
    const chat = rowChats[moveIndex(rowChats.length, at, step)];
    if (!chat) return;
    cursorId = chat.roomId;
    void tick().then(() => {
      const row = document.querySelector<HTMLElement>(`[data-room="${CSS.escape(chat.roomId)}"] .face`);
      row?.focus({ preventScroll: true });
      row?.scrollIntoView({ block: 'nearest' });
    });
  }

  function onKey(event: KeyboardEvent): void {
    const target = event.target as HTMLElement | null;
    const typing = target?.tagName === 'INPUT' || target?.tagName === 'TEXTAREA';
    const mod = event.metaKey || event.ctrlKey;
    if (mod && !event.altKey) {
      if (event.key.toLowerCase() === 'k') {
        event.preventDefault();
        void openSearch();
        return;
      }
      const digit = Number(event.key);
      const pinned = pinnedOf(chats.active)[digit - 1];
      if (digit >= 1 && digit <= 9 && pinned) {
        event.preventDefault();
        open(pinned);
      }
      return;
    }
    if (typing || menu) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      moveCursor(event.key === 'ArrowDown' ? 1 : -1);
    } else if (event.key === 'Enter' && (target === document.body || target === null)) {
      const chat = rowChats.find((c) => c.roomId === cursorId);
      if (chat) {
        event.preventDefault();
        open(chat);
      }
    }
  }

  // Dev-only screenshot switches: #/?search=1 opens search, #/?menu=1 opens the menu on the first row.
  let devMenuShown = false;
  $effect(() => {
    if (!devParams || devMenuShown || rowChats.length === 0) return;
    devMenuShown = true;
    if (devParams.has('search')) void openSearch();
    if (devParams.has('menu')) menu = { chat: rowChats[0] as Chat, x: 150, y: 200 };
  });

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    window.addEventListener('keydown', onKey);
    return () => {
      clearInterval(timer);
      window.removeEventListener('keydown', onKey);
      if (noticeTimer !== undefined) clearTimeout(noticeTimer);
    };
  });
</script>

{#snippet trailing()}
  {#if !archivedView}
    <IconButton icon="grid" label="Group by network" onclick={toggleGroup} size={24} pressed={grouped} />
  {/if}
  <IconButton icon="search" label="Search chats" onclick={openSearch} size={24} />
  <button class="me" type="button" aria-label="Settings" onclick={() => router.go('settings')}>
    <Avatar name="You" id="me" size={36} />
  </button>
{/snippet}

<section class="inbox">
  <Banner
    networks={archivedView ? undefined : bannerNetworks}
    colors={archivedView ? ['var(--banner-neutral)'] : undefined}
    title={archivedView ? 'Archived' : 'Chats'}
    status={searching ? '' : notice || (unread > 0 ? `${unread} unread` : '')}
    action={archivedView ? { label: 'Back', onclick: toggleArchived } : undefined}
    {trailing}
  >
    {#if searching}
      <input
        bind:this={searchEl}
        bind:value={query}
        class="search t-headline"
        type="search"
        placeholder="Search chats"
        aria-label="Search chats"
        onkeydown={onSearchKey}
      />
    {/if}
  </Banner>

  {#if stripNetworks.length > 0 && !archivedView}
    <NetworkStrip networks={stripNetworks} selected={network} nameOf={networkName} {allFill} onselect={selectNetwork} />
  {/if}

  {#if archivedView && source.length === 0}
    <div class="empty"><p class="t-body t-muted">No archived chats.</p></div>
  {:else if source.length === 0}
    <div class="empty">
      <p class="t-body">{accounts.list.length === 0 ? 'Add a network to see your chats here.' : 'No chats yet.'}</p>
      <Button onclick={() => router.go('add-network')}>Add a network</Button>
    </div>
  {:else if shown.length === 0}
    <div class="empty"><p class="t-body t-muted">No chats match.</p></div>
  {:else}
    <div class="list">
      <VirtualList items={items} keyOf={(item) => item.key} label="Chats" estimate={68}>
        {#snippet row(item: ListItem)}
          {#if item.kind === 'band'}
            <SectionBanner color={bannerCss(item.network)} label={item.label} />
          {:else}
            <ChatRow
              chat={item.chat}
              {now}
              selected={ui.mode === 'split' && ui.selectedRoomId === item.chat.roomId}
              cursor={cursorId === item.chat.roomId}
              onopen={open}
              onmenu={(chat, x, y) => (menu = { chat, x, y })}
              onswipe={(chat, action) => void act(chat, action)}
            />
          {/if}
        {/snippet}
      </VirtualList>
    </div>
  {/if}

  {#if !archivedView && chats.archived.length > 0}
    <button class="archived t-body" type="button" onclick={toggleArchived}>
      <Icon name="archive" size={20} />
      <span>Archived ({chats.archived.length})</span>
    </button>
  {/if}

  {#if menu}
    <RowMenu
      chat={menu.chat}
      x={menu.x}
      y={menu.y}
      {now}
      onaction={(action) => {
        const target = menu?.chat;
        menu = null;
        if (target) void act(target, action);
      }}
      onclose={() => (menu = null)}
    />
  {/if}
</section>

<style>
  .inbox {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
  }

  .list {
    flex: 1;
    min-height: 0;
    padding-top: var(--s-2);
  }

  .me {
    display: grid;
    place-items: center;
    width: var(--hit);
    height: var(--hit);
    border-radius: 50%;
  }

  .search {
    width: 100%;
    height: var(--hit);
    padding: 0 var(--s-4);
    border: 0;
    border-radius: var(--r-pill);
    background: rgb(255 255 255 / 0.55);
    color: var(--on-banner);
    font-weight: 600;
    animation: search-in var(--dur-quick) var(--ease-out) both;
  }

  @keyframes search-in {
    from {
      opacity: 0;
      transform: translateY(-4px);
    }
  }

  .search::placeholder {
    color: color-mix(in srgb, var(--on-banner) 70%, transparent);
  }

  /* The quiet footer: one row at the bottom of the list, muted, never a banner. */
  .archived {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    min-height: var(--hit);
    padding: 0 var(--gutter);
    color: var(--on-muted);
    font-weight: 600;
    text-align: start;
    border-top: 1px solid var(--outline);
  }

  @media (hover: hover) and (pointer: fine) {
    .archived:hover {
      color: var(--on);
    }
  }

  .empty {
    display: grid;
    justify-items: center;
    align-content: center;
    gap: var(--s-4);
    flex: 1;
    padding: var(--s-8) var(--gutter);
    text-align: center;
  }
</style>
