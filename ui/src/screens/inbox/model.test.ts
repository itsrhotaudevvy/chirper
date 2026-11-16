import { describe, expect, it } from 'vitest';
import type { Chat } from '../../lib/core';
import {
  formatRowTime,
  groupByNetwork,
  groupByServer,
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

  it('filters out servers/spaces and server channels from direct chats inbox', () => {
    const withServerAndSpace = [
      ...list,
      chat('Discord Guild', 'discord', { isSpace: true }),
      chat('general', 'discord', { parentId: '!guild:chirper.local', serverName: 'Discord Guild', isChannel: true }),
      chat('announcements', 'fluxer', { parentId: '!fguild:chirper.local', serverName: 'Fluxer Guild', isChannel: true }),
    ];
    // In 'chats' tab: server channels are filtered out
    expect(visibleChats(withServerAndSpace, null, '', 'chats').map((c) => c.roomId)).toEqual([
      'Alice',
      'Telegram team',
      'alice work',
    ]);
    // In 'servers' tab: only server channels are kept
    expect(visibleChats(withServerAndSpace, null, '', 'servers').map((c) => c.roomId)).toEqual([
      'general',
      'announcements',
    ]);
    // Can filter servers by network
    expect(visibleChats(withServerAndSpace, 'fluxer', '', 'servers').map((c) => c.roomId)).toEqual(['announcements']);
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

describe('groupByServer', () => {
  it('groups channels under their respective servers', () => {
    const list = [
      chat('c1', 'discord', { parentId: 'guild-1', serverName: 'Gaming' }),
      chat('c2', 'discord', { parentId: 'guild-1', serverName: 'Gaming' }),
      chat('c3', 'fluxer', { parentId: 'guild-2', serverName: 'Fluxer Devs' }),
    ];
    const groups = groupByServer(list);
    expect(groups.length).toBe(2);
    expect(groups[0]?.serverName).toBe('Gaming');
    expect(groups[0]?.chats.map((c) => c.roomId)).toEqual(['c1', 'c2']);
    expect(groups[1]?.serverName).toBe('Fluxer Devs');
    expect(groups[1]?.chats.map((c) => c.roomId)).toEqual(['c3']);
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
