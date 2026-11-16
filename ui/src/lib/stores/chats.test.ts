import { describe, expect, it, vi } from 'vitest';
import type { Chat } from '../core';
import { ChatsStore, createBadgeSync, precedes } from './chats.svelte';

function chat(roomId: string, lastActivity: number, extra: Partial<Chat> = {}): Chat {
  return {
    roomId,
    network: 'whatsapp',
    loginId: 'demo',
    name: roomId,
    avatarUrl: '',
    topic: '',
    isDm: true,
    isGroup: false,
    isSpace: false,
    memberCount: 2,
    lastActivity,
    preview: null,
    unreadCount: 0,
    mentionCount: 0,
    pinned: false,
    archived: false,
    mutedUntil: 0,
    typing: [],
    ...extra,
  };
}

describe('chat order', () => {
  it('puts pinned chats first, then most recent activity', () => {
    expect(precedes(chat('a', 1, { pinned: true }), chat('b', 9))).toBe(true);
    expect(precedes(chat('a', 9), chat('b', 1))).toBe(true);
  });

  it('inserts and re-positions chats incrementally', () => {
    const store = new ChatsStore({} as never);
    store.upsert(chat('old', 100));
    store.upsert(chat('new', 300));
    store.upsert(chat('mid', 200));
    expect(store.active.map((c) => c.roomId)).toEqual(['new', 'mid', 'old']);

    store.upsert(chat('old', 400));
    expect(store.active.map((c) => c.roomId)).toEqual(['old', 'new', 'mid']);

    store.upsert(chat('mid', 200, { pinned: true, unreadCount: 3 }));
    expect(store.active.map((c) => c.roomId)).toEqual(['mid', 'old', 'new']);
    expect(store.unreadTotal).toBe(3);
  });

  it('keeps archived chats out of the active list', () => {
    const store = new ChatsStore({} as never);
    store.upsert(chat('live', 1));
    store.upsert(chat('gone', 2, { archived: true }));
    expect(store.active.map((c) => c.roomId)).toEqual(['live']);
    expect(store.archived.map((c) => c.roomId)).toEqual(['gone']);
  });
});

describe('badge sync', () => {
  it('sends only the last count after the debounce window', async () => {
    vi.useFakeTimers();
    try {
      const sent: number[] = [];
      const sync = createBadgeSync(async (count) => void sent.push(count), 300);
      sync.update(1);
      vi.advanceTimersByTime(100);
      sync.update(4);
      vi.advanceTimersByTime(299);
      expect(sent).toEqual([]);
      vi.advanceTimersByTime(1);
      expect(sent).toEqual([4]);
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not resend an unchanged count and cancels on dispose', () => {
    vi.useFakeTimers();
    try {
      const sent: number[] = [];
      const sync = createBadgeSync(async (count) => void sent.push(count));
      sync.update(2);
      vi.advanceTimersByTime(300);
      sync.update(2);
      vi.advanceTimersByTime(300);
      sync.update(9);
      sync.dispose();
      vi.advanceTimersByTime(1000);
      expect(sent).toEqual([2]);
    } finally {
      vi.useRealTimers();
    }
  });
});
