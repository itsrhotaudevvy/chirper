<script lang="ts">
  /* Chat options: pin, mute, archive, mark unread, and search within this chat. */
  import type { Chat, Message } from '../../lib/core';
  import { core } from '../../lib/core';
  import Icon from '../../lib/components/Icon.svelte';
  import Sheet from '../../lib/components/Sheet.svelte';
  import { timeLabel } from './grouping';

  interface Props {
    chat: Chat;
    onclose: () => void;
    onerror: (message: string) => void;
    /** Search result tapped: the chat jumps to that message. */
    onjump: (eventId: string) => void;
  }

  let { chat, onclose, onerror, onjump }: Props = $props();

  /* Mute is "until forever" in the core: a far-future timestamp. 0 means not muted. */
  const FOREVER = Number.MAX_SAFE_INTEGER;
  let view = $state<'menu' | 'search'>('menu');
  let query = $state('');
  let results = $state<Message[]>([]);
  let timer: ReturnType<typeof setTimeout> | undefined;
  const muted = $derived(chat.mutedUntil > Date.now());

  async function flag(flags: { pinned?: boolean; archived?: boolean; mutedUntil?: number }): Promise<void> {
    try {
      await core.setChatFlags({ roomId: chat.roomId, flags });
      onclose();
    } catch (err: unknown) {
      onerror(err instanceof Error ? err.message : 'Could not change the chat');
    }
  }

  async function markUnread(): Promise<void> {
    try {
      await core.markUnread({ roomId: chat.roomId, unread: chat.unreadCount === 0 });
      onclose();
    } catch (err: unknown) {
      onerror(err instanceof Error ? err.message : 'Could not change the chat');
    }
  }

  function search(text: string): void {
    query = text;
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const q = query.trim();
      if (!q) {
        results = [];
        return;
      }
      const found = await core.search({ query: q, limit: 100 }).catch(() => [] as Message[]);
      results = found.filter((m) => m.roomId === chat.roomId);
    }, 200);
  }
</script>

<Sheet open title={view === 'search' ? 'Search in chat' : chat.name} {onclose}>
  {#if view === 'menu'}
    <ul class="rows">
      <li>
        <button class="row" type="button" onclick={() => flag({ pinned: !chat.pinned })}>
          <Icon name="pin" /> {chat.pinned ? 'Unpin' : 'Pin'}
        </button>
      </li>
      <li>
        <button class="row" type="button" onclick={() => flag({ mutedUntil: muted ? 0 : FOREVER })}>
          <Icon name="mute" /> {muted ? 'Unmute' : 'Mute'}
        </button>
      </li>
      <li>
        <button class="row" type="button" onclick={() => flag({ archived: !chat.archived })}>
          <Icon name="archive" /> {chat.archived ? 'Unarchive' : 'Archive'}
        </button>
      </li>
      <li>
        <button class="row" type="button" onclick={markUnread}>
          <Icon name="check" /> {chat.unreadCount > 0 ? 'Mark as read' : 'Mark as unread'}
        </button>
      </li>
      <li>
        <button class="row" type="button" onclick={() => (view = 'search')}>
          <Icon name="search" /> Search in chat
        </button>
      </li>
    </ul>
  {:else}
    <input
      class="field t-body"
      type="search"
      placeholder="Search this chat"
      aria-label="Search this chat"
      value={query}
      oninput={(e) => search(e.currentTarget.value)}
    />
    <ul class="results">
      {#each results as r (r.eventId)}
        <li>
          <button class="result" type="button" onclick={() => onjump(r.eventId)}>
            <span class="result-meta t-label">{r.fromMe ? 'You' : r.senderName} · {timeLabel(r.ts)}</span>
            <span class="result-body t-body truncate">{r.body || `[${r.type}]`}</span>
          </button>
        </li>
      {:else}
        {#if query.trim()}<li class="empty t-label">No messages found</li>{/if}
      {/each}
    </ul>
  {/if}
</Sheet>

<style>
  .rows {
    display: grid;
    gap: 2px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    min-height: var(--hit);
    padding: 0 var(--s-2);
    border-radius: var(--r-sm);
    font-weight: 600;
    text-align: start;
  }

  @media (hover: hover) and (pointer: fine) {
    .row:hover {
      background: var(--surface-high);
    }
  }

  .field {
    width: 100%;
    min-height: var(--hit);
    margin-bottom: var(--s-3);
    padding: 0 var(--s-3);
    border-radius: var(--r-pill);
    background: var(--surface-high);
    border: 0;
    outline: none;
    color: var(--on);
    user-select: text;
  }

  .results {
    display: grid;
    gap: var(--s-2);
  }

  .result {
    display: flex;
    flex-direction: column;
    width: 100%;
    min-height: var(--hit);
    padding: var(--s-2) var(--s-3);
    border-radius: var(--r-sm);
    background: var(--surface-high);
    min-width: 0;
    text-align: start;
  }

  .result:focus-visible {
    outline-offset: -2px;
  }

  .result-meta {
    color: var(--on-muted);
  }

  .empty {
    padding: var(--s-2);
    color: var(--on-muted);
  }
</style>
