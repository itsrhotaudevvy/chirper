import { describe, expect, it } from 'vitest';
import type { Chat } from '../../lib/core';
import {
  formatRowTime,
  groupByNetwork,
  moveIndex,
  networksOf,
  pinnedOf,
  previewText,
  unreadOf,
  visibleChats,
} from './model';

function chat(roomId: string, network: string, extra: Partial<Chat> = {}): Chat {
  return {
    roomId,
    network,
    loginId: 'demo',
    name: roomId,
    avatarUrl: '',
    topic: '',
    isDm: true,
    isGroup: false,
    isSpace: false,
    memberCount: 2,
    lastActivity: 1,
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

describe('visibleChats', () => {
  const list = [chat('Alice', 'whatsapp'), chat('Telegram team', 'telegram'), chat('alice work', 'slack')];

  it('keeps list order and filters by network', () => {
    expect(visibleChats(list, 'whatsapp', '').map((c) => c.roomId)).toEqual(['Alice']);
    expect(visibleChats(list, null, '').map((c) => c.roomId)).toEqual(['Alice', 'Telegram team', 'alice work']);
  });

  it('matches the name live, ignoring case and surrounding spaces', () => {
    expect(visibleChats(list, null, '  ALICE ').map((c) => c.roomId)).toEqual(['Alice', 'alice work']);
    expect(visibleChats(list, 'telegram', 'alice')).toEqual([]);
  });

  it('filters out servers/spaces from the inbox', () => {
    const withSpace = [...list, chat('Discord Server', 'discord', { isSpace: true })];
    expect(visibleChats(withSpace, null, '').map((c) => c.roomId)).toEqual(['Alice', 'Telegram team', 'alice work']);
  });
});

describe('networksOf and groupByNetwork', () => {
  it('lists networks in spectrum order (red first, pink last), once each', () => {
    const list = [
      chat('a', 'messenger'),
      chat('b', 'discord'),
      chat('c', 'whatsapp'),
      chat('d', 'instagram'),
      chat('e', 'discord'),
    ];
    expect(networksOf(list)).toEqual(['instagram', 'whatsapp', 'discord', 'messenger']);
  });

  it('groups in spectrum order and keeps each group in list order', () => {
    const list = [chat('p', 'telegram', { pinned: true }), chat('w', 'whatsapp'), chat('t', 'telegram')];
    const groups = groupByNetwork(list);
    expect(groups.map((g) => g.network)).toEqual(['whatsapp', 'telegram']);
    expect(groups[1]?.chats.map((c) => c.roomId)).toEqual(['p', 't']);
  });
});

describe('counts and pins', () => {
  it('sums unread counts and lists pinned chats in order', () => {
    const list = [chat('a', 'signal', { unreadCount: 2, pinned: true }), chat('b', 'signal', { unreadCount: 5 })];
    expect(unreadOf(list)).toBe(7);
    expect(pinnedOf(list).map((c) => c.roomId)).toEqual(['a']);
  });
});

describe('formatRowTime', () => {
  const now = new Date(2026, 10, 16, 15, 0).getTime();

  it('shows the clock for today, a weekday within the week, a date before that', () => {
    expect(formatRowTime(new Date(2026, 10, 16, 9, 5).getTime(), now)).toMatch(/09:05|9:05/);
    expect(formatRowTime(new Date(2026, 10, 12, 9, 5).getTime(), now)).toMatch(/\w/);
    expect(formatRowTime(new Date(2026, 10, 2, 9, 5).getTime(), now)).toMatch(/Nov|2/);
    expect(formatRowTime(0, now)).toBe('');
  });
});

describe('previewText', () => {
  it('prefixes groups with the first name of the sender only', () => {
    const group = chat('g', 'signal', { isGroup: true, preview: { sender: 'Priya Shah', body: 'Ship it', type: 'text' } });
    const dm = chat('d', 'signal', { preview: { sender: 'Sam', body: 'hi', type: 'text' } });
    expect(previewText(group)).toBe('Priya: Ship it');
    expect(previewText(dm)).toBe('hi');
    expect(previewText(chat('e', 'signal'))).toBe('');
  });
});

describe('moveIndex', () => {
  it('moves within the list and clamps at both ends', () => {
    expect(moveIndex(3, -1, 1)).toBe(0);
    expect(moveIndex(3, 0, -1)).toBe(0);
    expect(moveIndex(3, 2, 1)).toBe(2);
    expect(moveIndex(0, -1, 1)).toBe(-1);
  });
});
