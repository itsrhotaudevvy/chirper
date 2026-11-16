import type { Chat, CoreApi, CoreEvent } from '../core';
import { core as defaultCore } from '../core';
import { SvelteMap } from 'svelte/reactivity';

/** Pinned chats first, then most recent activity first; roomId breaks ties. */
export function precedes(a: Chat, b: Chat): boolean {
  if (a.pinned !== b.pinned) return a.pinned;
  if (a.lastActivity !== b.lastActivity) return a.lastActivity > b.lastActivity;
  return a.roomId < b.roomId;
}

/** Index at which `chat` must be inserted into `order` (which is already sorted). */
function insertionIndex(order: readonly string[], chat: Chat, byId: ReadonlyMap<string, Chat>): number {
  let lo = 0;
  let hi = order.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    const other = byId.get(order[mid] ?? '');
    if (other && precedes(other, chat)) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * Chats keyed by roomId, kept in display order. Events are applied incrementally: an update that
 * keeps pinned and lastActivity moves nothing; otherwise the chat is re-inserted by binary search.
 */
export class ChatsStore {
  readonly #byId = new SvelteMap<string, Chat>();
  #order = $state<string[]>([]);

  /** Non-archived chats in display order. */
  readonly active = $derived(
    this.#order
      .filter((id) => {
        const c = this.#byId.get(id);
        return c && !c.archived && !c.isSpace;
      })
      .map((id) => this.#byId.get(id) as Chat),
  );
  /** Archived chats, same order. */
  readonly archived = $derived(
    this.#order
      .filter((id) => {
        const c = this.#byId.get(id);
        return c && c.archived && !c.isSpace;
      })
      .map((id) => this.#byId.get(id) as Chat),
  );
  readonly unreadTotal = $derived(this.active.reduce((sum, c) => sum + c.unreadCount, 0));

  constructor(private readonly api: CoreApi = defaultCore) {}

  get(roomId: string): Chat | undefined {
    return this.#byId.get(roomId);
  }

  /** Loads active and archived chats from the core, replacing the current set. */
  async load(): Promise<void> {
    const base = { network: '', unreadOnly: false, query: '' };
    const [live, archived] = await Promise.all([
      this.api.listChats({ filter: { ...base, archived: false } }),
      this.api.listChats({ filter: { ...base, archived: true } }),
    ]);
    this.#byId.clear();
    for (const chat of [...live, ...archived]) this.#byId.set(chat.roomId, chat);
    this.#order = [...this.#byId.values()].sort((a, b) => (precedes(a, b) ? -1 : 1)).map((c) => c.roomId);
  }

  /** Applies core events (chat, chat_removed, typing). Other kinds are ignored here. */
  apply(events: readonly CoreEvent[]): void {
    for (const event of events) {
      if (event.kind === 'chat') this.upsert(event.payload);
      else if (event.kind === 'chat_removed') this.remove(event.payload.roomId);
      else if (event.kind === 'typing') this.setTyping(event.payload.roomId, event.payload.names);
    }
  }

  upsert(chat: Chat): void {
    const previous = this.#byId.get(chat.roomId);
    this.#byId.set(chat.roomId, chat);
    if (previous && previous.pinned === chat.pinned && previous.lastActivity === chat.lastActivity) return;
    if (previous) {
      this.#order = this.#order.filter((id) => id !== chat.roomId);
    }
    const at = insertionIndex(this.#order, chat, this.#byId);
    this.#order.splice(at, 0, chat.roomId);
  }

  remove(roomId: string): void {
    this.#byId.delete(roomId);
    this.#order = this.#order.filter((id) => id !== roomId);
  }

  private setTyping(roomId: string, names: string[]): void {
    const chat = this.#byId.get(roomId);
    if (chat) this.#byId.set(roomId, { ...chat, typing: names });
  }
}

export const chats = new ChatsStore();

/**
 * Sends the unread total to the app badge. Changes are coalesced: only the last count is sent,
 * `delayMs` after the last change. The first call is also delayed, so start-up bursts do not flicker.
 */
export function createBadgeSync(
  setBadge: (count: number) => Promise<void>,
  delayMs = 300,
): { update: (count: number) => void; dispose: () => void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let sent: number | undefined;
  const cancel = (): void => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  return {
    update(count: number): void {
      cancel();
      timer = setTimeout(() => {
        timer = undefined;
        if (count === sent) return;
        sent = count;
        setBadge(count).catch(() => undefined);
      }, delayMs);
    },
    dispose: cancel,
  };
}
