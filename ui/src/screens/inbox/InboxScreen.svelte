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
  import { updater, type UpdaterState } from '../../lib/updater';
  import {
    MUTE_FOREVER,
    groupByNetwork,
    groupByServer,
    isDirectChat,
    isServerChannel,
    moveIndex,
    networksOf,
    pinnedOf,
    unreadOf,
    visibleChats,
    type ChatAction,
    type InboxTab,
    type ListItem,
  } from './model';

  /** Dev-only screenshot switches, read from the hash: #/?net=telegram or #/?group=1. */
  const devParams = import.meta.env.DEV ? new URLSearchParams(location.hash.split('?')[1] ?? '') : null;

  let tab = $state<InboxTab>('chats');
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
  const directChats = $derived(source.filter(isDirectChat));
  const serverChats = $derived(source.filter(isServerChannel));
  const directUnread = $derived(unreadOf(directChats));
  const serverUnread = $derived(unreadOf(serverChats));

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
  const shown = $derived(visibleChats(source, network, query, tab));
  const unread = $derived(unreadOf(shown));

  const items = $derived.by<ListItem[]>(() => {
    if (tab === 'servers') {
      return groupByServer(shown).flatMap(({ serverKey, serverName, network: id, chats: list }) => [
        { key: `server:${serverKey}`, kind: 'server', serverKey, network: id, label: serverName },
        ...list.map((chat): ListItem => ({ key: `row:${chat.roomId}`, kind: 'row', chat })),
      ]);
    }
    if (!grouped) return shown.map((chat): ListItem => ({ key: `row:${chat.roomId}`, kind: 'row', chat }));
    return groupByNetwork(shown).flatMap(({ network: id, chats: list }) => [
      { key: `band:${id}`, kind: 'band', network: id, label: networkName(id) },
      ...list.map((chat): ListItem => ({ key: `row:${chat.roomId}`, kind: 'row', chat })),
    ]);
  });
  /** Chats in the order they are drawn, for the keyboard cursor. */
  const rowChats = $derived(items.flatMap((item) => (item.kind === 'row' ? [item.chat] : [])));

  function selectTab(next: InboxTab): void {
    if (tab === next) return;
    tab = next;
    haptic('selection');
  }

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
  let updateState = $state<UpdaterState>(updater.state);
  $effect(() => {
    if (!devParams || devMenuShown || rowChats.length === 0) return;
    devMenuShown = true;
    if (devParams.has('search')) void openSearch();
    if (devParams.has('menu')) menu = { chat: rowChats[0] as Chat, x: 150, y: 200 };
  });

  onMount(() => {
    const timer = setInterval(() => (now = Date.now()), 30_000);
    const unsubUpdater = updater.subscribe((s) => (updateState = s));
    window.addEventListener('keydown', onKey);
    return () => {
      clearInterval(timer);
      unsubUpdater();
      window.removeEventListener('keydown', onKey);
      if (noticeTimer !== undefined) clearTimeout(noticeTimer);
    };
  });
</script>

{#snippet trailing()}
  {#if searching}
    <IconButton icon="close" label="Close search" onclick={closeSearch} size={24} />
  {:else}
    {#if !archivedView}
      <IconButton icon="grid" label="Group by network" onclick={toggleGroup} size={24} pressed={grouped} />
    {/if}
    <IconButton icon="search" label="Search chats" onclick={openSearch} size={24} />
    <button class="me" type="button" aria-label="Settings" onclick={() => router.go('settings')}>
      <Avatar name="You" id="me" size={36} />
    </button>
  {/if}
{/snippet}

<section class="inbox">
  <Banner
    networks={archivedView ? undefined : bannerNetworks}
    colors={archivedView ? ['var(--banner-neutral)'] : undefined}
    title={archivedView ? 'Archived' : tab === 'servers' ? 'Servers' : 'Chats'}
    hideTitle={searching}
    status={searching ? '' : notice || (unread > 0 ? `${unread} unread` : '')}
    action={archivedView ? { label: 'Back', onclick: toggleArchived } : undefined}
    titlebar
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

  {#if !archivedView}
    <div class="tabs-nav" role="tablist" aria-label="Inbox categories">
      <button
        class="tab-pill"
        class:active={tab === 'chats'}
        type="button"
        role="tab"
        aria-selected={tab === 'chats'}
        onclick={() => selectTab('chats')}
      >
        <span>Chats</span>
        {#if directUnread > 0}
          <span class="tab-count">{directUnread}</span>
        {/if}
      </button>
      <button
        class="tab-pill"
        class:active={tab === 'servers'}
        type="button"
        role="tab"
        aria-selected={tab === 'servers'}
        onclick={() => selectTab('servers')}
      >
        <span>Servers</span>
        {#if serverUnread > 0}
          <span class="tab-count">{serverUnread}</span>
        {/if}
      </button>
    </div>
  {/if}

  {#if (updateState.status === 'ready' || updateState.status === 'available') && !updateState.dismissed}
    <div class="update-banner">
      <span class="update-text t-label">
        {updateState.status === 'ready'
          ? `Chirper ${updateState.latestVersion} downloaded`
          : `Chirper ${updateState.latestVersion} available`}
      </span>
      <div class="update-btns">
        <button
          class="update-btn"
          type="button"
          onclick={() => void updater.download(updateState.downloadUrl, updateState.assetName)}
        >
          {updateState.status === 'ready' ? 'Install' : 'Download'}
        </button>
        <button class="update-dismiss" type="button" aria-label="Dismiss" onclick={() => updater.dismiss()}>×</button>
      </div>
    </div>
  {/if}

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
    <div class="empty">
      {#if query}
        <p class="t-body t-muted">No chats match.</p>
      {:else if tab === 'servers'}
        <p class="t-body t-muted">No server channels yet.</p>
      {:else}
        <p class="t-body t-muted">No direct chats yet.</p>
      {/if}
    </div>
  {:else}
    <div class="list">
      <VirtualList items={items} keyOf={(item) => item.key} label={tab === 'servers' ? 'Servers' : 'Chats'} estimate={68}>
        {#snippet row(item: ListItem)}
          {#if item.kind === 'band' || item.kind === 'server'}
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

  /* Desktop glass: a translucent main container with a soft blur behind it (settings: Blur chat containers). */
  :global(html[data-glass='true'][data-form='desktop']) .inbox {
    background: color-mix(in srgb, var(--bg) 78%, transparent);
    backdrop-filter: blur(24px) saturate(1.2);
    -webkit-backdrop-filter: blur(24px) saturate(1.2);
  }

  .list {
    flex: 1;
    min-height: 0;
    padding-top: var(--s-2);
  }

  .tabs-nav {
    display: flex;
    gap: var(--s-2);
    margin: var(--s-1) var(--gutter);
    padding: 2px;
    background: color-mix(in srgb, var(--on) 6%, transparent);
    border-radius: var(--r-pill);
    border: 1px solid var(--outline);
  }

  .tab-pill {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--s-2);
    height: 30px;
    padding: 0 var(--s-3);
    border: 0;
    border-radius: var(--r-pill);
    background: transparent;
    color: var(--on-muted);
    font-size: var(--t-headline-size);
    font-weight: 600;
    cursor: pointer;
    transition:
      background var(--dur-quick) var(--ease-out),
      color var(--dur-quick) var(--ease-out),
      box-shadow var(--dur-quick) var(--ease-out),
      transform var(--dur-quick) var(--ease-out);
  }

  .tab-pill:active {
    transform: scale(0.98);
  }

  .tab-pill.active {
    background: var(--surface);
    color: var(--on);
    box-shadow: 0 1px 3px rgb(0 0 0 / 0.12);
  }

  .tab-count {
    display: inline-grid;
    place-items: center;
    min-width: 18px;
    height: 18px;
    padding: 0 5px;
    border-radius: var(--r-pill);
    background: var(--accent);
    color: var(--on-banner);
    font-size: 11px;
    font-weight: 700;
    line-height: 1;
  }

  .update-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-2);
    margin: var(--s-1) var(--gutter);
    padding: var(--s-2) var(--s-3);
    border-radius: var(--r-md);
    background: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 40%, transparent);
  }

  .update-text {
    font-weight: 600;
    color: var(--on);
  }

  .update-btns {
    display: flex;
    align-items: center;
    gap: var(--s-2);
  }

  .update-btn {
    padding: 3px var(--s-3);
    border-radius: var(--r-pill);
    background: var(--accent);
    color: var(--on-banner);
    border: 0;
    font-size: var(--t-label-size);
    font-weight: 700;
    cursor: pointer;
  }

  .update-dismiss {
    background: transparent;
    border: 0;
    font-size: 18px;
    line-height: 1;
    color: var(--on-muted);
    cursor: pointer;
    padding: 0 4px;
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
    min-height: calc(var(--hit) + 6px);
    margin: var(--s-2) var(--gutter) calc(var(--s-3) + var(--safe-bottom));
    padding: var(--s-2) var(--s-4);
    border-radius: var(--r-lg);
    background: color-mix(in srgb, var(--on) 4%, transparent);
    color: var(--on-muted);
    font-weight: 600;
    text-align: start;
    border: 1px solid var(--outline);
    transition:
      background var(--dur-quick) var(--ease-out),
      color var(--dur-quick) var(--ease-out),
      transform var(--dur-quick) var(--ease-out);
  }

  .archived:active {
    transform: scale(0.98);
    background: color-mix(in srgb, var(--on) 8%, transparent);
  }

  @media (hover: hover) and (pointer: fine) {
    .archived:hover {
      color: var(--on);
      background: color-mix(in srgb, var(--on) 7%, transparent);
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
