<script lang="ts">
  /*
   * One chat: the banner header, the reverse timeline (virtual, anchored on prepend), the composer,
   * and the message actions. Screens never import each other; the back action is a prop.
   */
  import { onMount, tick, untrack } from 'svelte';
  import { SvelteMap, SvelteSet } from 'svelte/reactivity';
  import type { Message } from '../../lib/core';
  import { core } from '../../lib/core';
  import { chats, messages, ui } from '../../lib/stores';
  import { haptic } from '../../lib/haptics';
  import { bannerCss } from '../../lib/spectrum';
  import Button from '../../lib/components/Button.svelte';
  import Sheet from '../../lib/components/Sheet.svelte';
  import Skeleton from '../../lib/components/Skeleton.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import VirtualList from '../../lib/components/VirtualList.svelte';
  import ChatHeader from './ChatHeader.svelte';
  import Composer from './Composer.svelte';
  import EmojiSheet from './EmojiSheet.svelte';
  import ImageViewer from './ImageViewer.svelte';
  import MessageActions, { type MessageAction } from './MessageActions.svelte';
  import MessageRow from './MessageRow.svelte';
  import MoreSheet from './MoreSheet.svelte';
  import { buildRows, type MessageRow as Row, type TimelineRow } from './grouping';

  interface Props {
    roomId: string;
    /** Phone and tablet: return to the inbox. */
    onback?: () => void;
  }

  let { roomId, onback }: Props = $props();

  /** The header collapses over this many px of scrolling away from the newest message. */
  const COLLAPSE_PX = 112;
  /** Keyboard taller than this shrinks the chat to the visible viewport. */
  const KEYBOARD_PX = 120;
  const TOP: TimelineRow = { kind: 'top', key: '__top' };
  const HEART = '❤️';

  const chat = $derived(chats.get(roomId));
  const ctx = $derived(bannerCss(chat?.network ?? 'demo'));
  const timeline = $derived(messages.timeline(roomId));
  const groupChat = $derived(chat ? chat.isGroup || chat.isSpace : false);

  /*
   * Retries: a failed send is a local echo with status 'failed'. Once a retry is issued, the echo it
   * replaces is hidden, so the timeline shows only the new attempt. Attempts are tracked by txnId;
   * an echo whose send did not run in this session is tracked by its eventId.
   */
  const txnOf = new SvelteMap<string, string>();
  const superseded = new SvelteSet<string>();
  const attemptKey = (m: Message): string => txnOf.get(m.eventId) ?? m.eventId;
  const visible = $derived(timeline.items.filter((m) => !superseded.has(attemptKey(m))));

  /* Row objects are reused for unchanged messages, so the list does not re-render every row on each event. */
  const rowMemo = new Map<string, Row>();
  const rows = $derived<TimelineRow[]>([TOP, ...buildRows(visible, { groupChat, now: Date.now() }, rowMemo)]);
  const newest = $derived(visible[visible.length - 1]);
  const lastOwn = $derived(findLastOwn(visible));

  let progress = $state(0);
  /** The timeline, for scrolling a search result into view. */
  let list = $state<{ scrollToKey: (key: string) => boolean } | undefined>(undefined);
  /** The message a search just jumped to; it flashes once (DESIGN 5). */
  let flashId = $state<string | null>(null);
  let flashTimer: ReturnType<typeof setTimeout> | undefined;
  let replyTo = $state<Message | null>(null);
  let editing = $state<Message | null>(null);
  let actionFor = $state<Message | null>(null);
  let emojiFor = $state<Message | null>(null);
  let deleting = $state<Message | null>(null);
  let moreOpen = $state(false);
  let viewer = $state<{ src: string | null; kind: 'image' | 'video' } | null>(null);
  let notice = $state('');
  let noticeTimer: ReturnType<typeof setTimeout> | undefined;
  let composer = $state<{ focus: () => void } | undefined>(undefined);
  let windowFocused = $state(true);
  let newestVisible = $state(false);
  let keyboardHeight = $state<number | null>(null);
  let markedRead = '';

  function findLastOwn(list: readonly Message[]): Message | null {
    for (let i = list.length - 1; i >= 0; i--) {
      const m = list[i];
      if (m?.fromMe && m.type === 'text' && !m.redacted) return m;
    }
    return null;
  }

  function report(err: unknown): void {
    notice = err instanceof Error ? err.message : 'Something went wrong';
    clearTimeout(noticeTimer);
    noticeTimer = setTimeout(() => (notice = ''), 4000);
  }

  /** Older pages load until the message is in the timeline (bounded), then it is centred and flashes. */
  const MAX_PAGES_TO_JUMP = 40;
  async function jump(eventId: string): Promise<void> {
    for (let page = 0; page < MAX_PAGES_TO_JUMP; page++) {
      if (visible.some((m) => m.eventId === eventId) || !timeline.hasMore) break;
      await messages.loadOlder(roomId).catch(report);
    }
    await tick();
    if (!list?.scrollToKey(eventId)) {
      report(new Error('That message is not loaded'));
      return;
    }
    flashId = eventId;
    clearTimeout(flashTimer);
    flashTimer = setTimeout(() => (flashId = null), 900);
  }

  /* Load the newest page when the chat opens. Re-runs only when the room changes. */
  $effect(() => {
    const id = roomId;
    untrack(() => {
      messages.open(id).catch(report);
    });
  });

  /* Mark the newest message read once it is on screen and the window has focus. */
  $effect(() => {
    const id = newest?.eventId;
    if (!id || !newestVisible || !windowFocused || id === markedRead) return;
    markedRead = id;
    void core.markRead({ roomId, eventId: id }).catch(() => undefined);
  });

  onMount(() => {
    const onFocus = (): void => {
      windowFocused = true;
    };
    const onBlur = (): void => {
      windowFocused = false;
    };
    const vv = window.visualViewport;
    const onViewport = (): void => {
      if (!vv) return;
      const hidden = window.innerHeight - vv.height - vv.offsetTop;
      keyboardHeight = hidden > KEYBOARD_PX ? vv.height : null;
    };
    window.addEventListener('focus', onFocus);
    window.addEventListener('blur', onBlur);
    vv?.addEventListener('resize', onViewport);
    vv?.addEventListener('scroll', onViewport);
    return () => {
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('blur', onBlur);
      vv?.removeEventListener('resize', onViewport);
      vv?.removeEventListener('scroll', onViewport);
      clearTimeout(noticeTimer);
    };
  });

  /** Watches scrolling of the timeline (capture, because scroll does not bubble) to drive the header collapse. */
  function trackScroll(node: HTMLElement): { destroy: () => void } {
    const onScroll = (event: Event): void => {
      const el = event.target;
      if (!(el instanceof HTMLElement)) return;
      const away = el.scrollHeight - el.scrollTop - el.clientHeight;
      progress = Math.min(1, Math.max(0, away / COLLAPSE_PX));
    };
    node.addEventListener('scroll', onScroll, { capture: true, passive: true });
    return { destroy: () => node.removeEventListener('scroll', onScroll, { capture: true }) };
  }

  function rowKey(item: TimelineRow): string {
    return item.key;
  }

  function loadOlder(): void {
    if (!timeline.loaded) return;
    messages.loadOlder(roomId).catch(report);
  }

  function reply(m: Message): void {
    replyTo = m;
    editing = null;
    composer?.focus();
  }

  function startEdit(m: Message): void {
    editing = m;
    replyTo = null;
  }

  function cancelContext(): void {
    replyTo = null;
    editing = null;
  }

  function hasOwn(m: Message, key: string): boolean {
    return m.reactions.some((r) => r.key === key && r.fromMe);
  }

  function addReaction(m: Message, key: string): void {
    if (hasOwn(m, key)) return;
    core.react({ roomId, eventId: m.eventId, key }).catch(report);
  }

  function toggleReaction(m: Message, key: string): void {
    const call = hasOwn(m, key) ? core.unreact : core.react;
    call({ roomId, eventId: m.eventId, key }).catch(report);
  }

  function heart(m: Message): void {
    addReaction(m, HEART);
  }

  async function submit(text: string): Promise<void> {
    try {
      if (editing) {
        const target = editing;
        editing = null;
        if (text !== target.body) await core.edit({ roomId, eventId: target.eventId, text });
      } else {
        const target = replyTo;
        replyTo = null;
        const txnId = crypto.randomUUID();
        const sent = await core.sendText({ roomId, text, replyTo: target?.eventId ?? '', txnId });
        txnOf.set(sent.eventId, txnId);
      }
    } catch (err: unknown) {
      report(err);
    }
  }

  /** Sends a failed text again. The failed echo is hidden at once: only the new attempt shows. */
  function retry(m: Message): void {
    if (m.type !== 'text') return;
    superseded.add(attemptKey(m));
    const txnId = crypto.randomUUID();
    core
      .sendText({ roomId, text: m.body, replyTo: m.replyTo?.eventId ?? '', txnId })
      .then((sent) => txnOf.set(sent.eventId, txnId))
      .catch(report);
  }

  function onAction(action: MessageAction): void {
    const m = actionFor;
    actionFor = null;
    if (!m) return;
    switch (action) {
      case 'reply':
        reply(m);
        break;
      case 'copy':
        void navigator.clipboard?.writeText(m.body).catch(() => undefined);
        break;
      case 'edit':
        startEdit(m);
        break;
      case 'delete':
        deleting = m;
        break;
      case 'more-reactions':
        emojiFor = m;
        break;
    }
  }

  function confirmDelete(): void {
    const m = deleting;
    deleting = null;
    if (!m) return;
    haptic('warning');
    core.redact({ roomId, eventId: m.eventId }).catch(report);
  }

  function openViewer(src: string | null, kind: 'image' | 'video'): void {
    viewer = { src, kind };
  }
</script>

<section class="chat" style:--ctx={ctx} style:height={keyboardHeight ? `${keyboardHeight}px` : undefined} aria-label={chat?.name ?? 'Chat'}>
  <ChatHeader
    {chat}
    {ctx}
    {progress}
    showBack={ui.mode !== 'split'}
    onback={() => onback?.()}
    onmore={() => (moreOpen = true)}
  />

  <div class="timeline" use:trackScroll>
    {#if !timeline.loaded}
      <div class="loading" aria-hidden="true">
        <span class="top-space"></span>
        <Skeleton width="62%" height="14px" />
        <Skeleton width="38%" height="14px" />
        <Skeleton width="48%" height="14px" />
      </div>
    {:else}
      <VirtualList
        bind:this={list}
        items={rows}
        keyOf={rowKey}
        label="Messages"
        reverse
        estimate={64}
        onStart={loadOlder}
      >
        {#snippet row(item: TimelineRow)}
          {#if item.kind === 'top'}
            <div class="top-space" aria-hidden="true"></div>
          {:else if item.kind === 'day'}
            <div class="column"><p class="day t-label">{item.label}</p></div>
          {:else}
            <div class="column">
              <MessageRow
                row={item}
                onreply={reply}
                onmenu={(m) => (actionFor = m)}
                ontoggle={toggleReaction}
                onheart={heart}
                onopen={openViewer}
                onretry={retry}
                onnewest={(on) => (newestVisible = on)}
                flash={item.key === flashId}
              />
            </div>
          {/if}
        {/snippet}
      </VirtualList>
      {#if visible.length === 0}
        <p class="empty t-body">No messages yet. Say hello.</p>
      {/if}
    {/if}
  </div>

  {#if notice}
    <div class="notice"><StatusStrip tone="danger" message={notice} /></div>
  {/if}

  <Composer
    bind:this={composer}
    {roomId}
    reply={replyTo}
    edit={editing}
    {lastOwn}
    {ctx}
    onsubmit={submit}
    oncancel={cancelContext}
    onedit={startEdit}
    onerror={(message) => report(new Error(message))}
  />

  <MessageActions
    message={actionFor}
    onreact={(key) => {
      if (actionFor) addReaction(actionFor, key);
      actionFor = null;
    }}
    onaction={onAction}
    onclose={() => (actionFor = null)}
  />

  <EmojiSheet
    open={emojiFor !== null}
    title="React"
    onpick={(emoji) => {
      if (emojiFor) addReaction(emojiFor, emoji);
      emojiFor = null;
    }}
    onclose={() => (emojiFor = null)}
  />

  <Sheet open={deleting !== null} title="Delete message?" onclose={() => (deleting = null)}>
    <p class="t-body confirm">The message will show as deleted for everyone in this chat.</p>
    <div class="actions">
      <Button variant="quiet" onclick={() => (deleting = null)}>Cancel</Button>
      <Button color="var(--danger)" onclick={confirmDelete}>Delete</Button>
    </div>
  </Sheet>

  {#if moreOpen && chat}
    <MoreSheet
      {chat}
      onclose={() => (moreOpen = false)}
      onerror={(message) => report(new Error(message))}
      onjump={(eventId) => {
        moreOpen = false;
        void jump(eventId);
      }}
    />
  {/if}

  {#if viewer}
    <ImageViewer src={viewer.src} kind={viewer.kind} onclose={() => (viewer = null)} />
  {/if}
</section>

<style>
  .chat {
    position: relative;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    --head-big: var(--banner-h);
  }

  @media (min-width: 900px) {
    .chat {
      --head-big: var(--banner-h-desktop);
    }
  }

  .timeline {
    position: relative;
    flex: 1 1 auto;
    min-height: 0;
  }

  .top-space {
    display: block;
    height: var(--head-big);
  }

  /* The reading column: timeline and composer share 760 px, centred in wide panes. */
  .column {
    max-width: 760px;
    margin: 0 auto;
  }

  .loading {
    display: grid;
    gap: var(--s-3);
    padding: 0 var(--gutter);
  }

  .day {
    padding: var(--s-4) var(--gutter) var(--s-1);
    text-align: center;
    color: var(--on-muted);
  }

  .empty {
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    text-align: center;
    color: var(--on-muted);
    transform: translateY(-50%);
  }

  .notice {
    padding: 0 var(--gutter) var(--s-2);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--s-2);
    padding-top: var(--s-3);
  }

  .confirm {
    padding: 0 var(--s-2) var(--s-2);
    color: var(--on-muted);
  }
</style>
