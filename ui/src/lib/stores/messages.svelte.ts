import type { CoreApi, CoreEvent, Message } from '../core';
import { core as defaultCore } from '../core';
import { SvelteMap } from 'svelte/reactivity';

/** Messages fetched per page. */
export const PAGE_SIZE = 50;

/** The loaded window of one room, oldest first (newest last). */
export class RoomTimeline {
  items = $state.raw<Message[]>([]);
  hasMore = $state(true);
  loaded = $state(false);
  loading = $state(false);
}

/** Returned for rooms that were never opened. Never written to: loads go through #ensure. */
const EMPTY_TIMELINE = new RoomTimeline();

/** Inserts or replaces `message` in an order-sorted list. */
export function withMessage(list: readonly Message[], message: Message): Message[] {
  const index = list.findIndex((m) => m.eventId === message.eventId);
  if (index >= 0) return list.map((m, i) => (i === index ? message : m));
  let at = list.length;
  while (at > 0 && (list[at - 1]?.order ?? 0) > message.order) at--;
  return [...list.slice(0, at), message, ...list.slice(at)];
}

/** Messages of every opened room, with paging (loadOlder) and live updates. */
export class MessagesStore {
  readonly #rooms = new SvelteMap<string, RoomTimeline>();

  private readonly api: CoreApi;

  constructor(api: CoreApi = defaultCore) {
    this.api = api;
  }

  /**
   * Read-only view of a room. Safe to call from templates and $derived: it never writes state.
   * Rooms that were never opened read as an empty, unloaded timeline.
   */
  timeline(roomId: string): RoomTimeline {
    return this.#rooms.get(roomId) ?? EMPTY_TIMELINE;
  }

  /** The room's mutable timeline, created on first use. Only called from actions, never from reads. */
  #ensure(roomId: string): RoomTimeline {
    let room = this.#rooms.get(roomId);
    if (!room) {
      room = new RoomTimeline();
      this.#rooms.set(roomId, room);
    }
    return room;
  }

  /** Loads the newest page of a room the first time it is opened. */
  async open(roomId: string): Promise<void> {
    const room = this.#ensure(roomId);
    if (room.loaded || room.loading) return;
    room.loading = true;
    try {
      const page = await this.api.listMessages({ roomId, beforeOrder: 0, limit: PAGE_SIZE });
      room.items = page;
      room.hasMore = true;
      room.loaded = true;
    } finally {
      room.loading = false;
    }
  }

  /** Fetches one older page and prepends it. A no-op while a page is already loading. */
  async loadOlder(roomId: string): Promise<void> {
    const room = this.#ensure(roomId);
    if (room.loading || !room.hasMore) return;
    room.loading = true;
    try {
      const oldest = room.items[0]?.order ?? 0;
      let page = await this.api.listMessages({ roomId, beforeOrder: oldest, limit: PAGE_SIZE });
      let hasMore = true;
      if (page.length === 0) {
        const res = await this.api.loadOlder({ roomId });
        hasMore = res.hasMore;
        page = await this.api.listMessages({ roomId, beforeOrder: oldest, limit: PAGE_SIZE });
      }
      const known = new Set(room.items.map((m) => m.eventId));
      const fresh = page.filter((m) => !known.has(m.eventId));
      if (fresh.length > 0) {
        room.items = [...fresh, ...room.items].sort((a, b) => a.order - b.order);
      }
      room.hasMore = hasMore;
    } finally {
      room.loading = false;
    }
  }

  /** Applies message and message_removed events to the rooms that are loaded. */
  apply(events: readonly CoreEvent[]): void {
    for (const event of events) {
      if (event.kind === 'message') {
        const room = this.#rooms.get(event.payload.roomId);
        if (room?.loaded) room.items = withMessage(room.items, event.payload);
      } else if (event.kind === 'message_removed') {
        const room = this.#rooms.get(event.payload.roomId);
        if (room?.loaded) room.items = room.items.filter((m) => m.eventId !== event.payload.eventId);
      }
    }
  }
}

export const messages = new MessagesStore();
