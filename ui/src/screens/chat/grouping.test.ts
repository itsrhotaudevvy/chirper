import { describe, expect, it } from 'vitest';
import type { Message } from '../../lib/core';
import { buildRows, emojiOnlyCount, GROUP_GAP_MS, jumboSize, sameGroup, type MessageRow } from './grouping';

const DAY = 86_400_000;
const NOON = new Date(2026, 10, 16, 12, 0, 0).getTime();

function msg(id: number, overrides: Partial<Message> = {}): Message {
  return {
    eventId: `$e${id}`,
    roomId: '!r:x',
    order: id,
    sender: '@a:x',
    senderName: 'Ana',
    senderAvatar: '',
    fromMe: false,
    ts: NOON + id * 1000,
    type: 'text',
    body: `m${id}`,
    formattedBody: '',
    media: null,
    replyTo: null,
    edited: false,
    redacted: false,
    reactions: [],
    status: 'read',
    error: '',
    live: false,
    ...overrides,
  };
}

function messageRows(rows: ReturnType<typeof buildRows>): MessageRow[] {
  return rows.filter((r): r is MessageRow => r.kind === 'msg');
}

describe('emojiOnlyCount', () => {
  it('counts one to three emoji, ignoring spaces', () => {
    expect(emojiOnlyCount('😂')).toBe(1);
    expect(emojiOnlyCount('😂 😂')).toBe(2);
    expect(emojiOnlyCount(' 🔥🔥🔥 ')).toBe(3);
    expect(emojiOnlyCount('❤️')).toBe(1);
    expect(emojiOnlyCount('👨‍👩‍👧')).toBe(1);
  });

  it('returns 0 for text, mixed text, four emoji or nothing', () => {
    expect(emojiOnlyCount('hi 😂')).toBe(0);
    expect(emojiOnlyCount('😂😂😂😂')).toBe(0);
    expect(emojiOnlyCount('   ')).toBe(0);
    expect(emojiOnlyCount('')).toBe(0);
    expect(emojiOnlyCount('1')).toBe(0);
  });
});

describe('jumboSize', () => {
  it('is 52 px for one or two emoji and 40 px for three', () => {
    expect(jumboSize(msg(1, { body: '🎉' }))).toBe(52);
    expect(jumboSize(msg(2, { body: '🎉🎉' }))).toBe(52);
    expect(jumboSize(msg(3, { body: '🎉🎉🎉' }))).toBe(40);
  });

  it('is null for replies, media, redacted and non-text messages', () => {
    expect(jumboSize(msg(1, { body: '🎉', replyTo: { eventId: '$x', senderName: 'B', body: 'q' } }))).toBeNull();
    expect(jumboSize(msg(2, { body: '🎉', redacted: true }))).toBeNull();
    expect(jumboSize(msg(3, { body: '🎉', type: 'image' }))).toBeNull();
    expect(jumboSize(msg(4, { body: 'hello' }))).toBeNull();
  });
});

describe('sameGroup', () => {
  it('groups the same sender within the gap, on the same day', () => {
    const a = msg(1);
    expect(sameGroup(a, msg(2))).toBe(true);
    expect(sameGroup(a, msg(2, { sender: '@b:x' }))).toBe(false);
    expect(sameGroup(a, msg(2, { ts: a.ts + GROUP_GAP_MS + 1 }))).toBe(false);
    expect(sameGroup(undefined, a)).toBe(false);
  });

  it('breaks a group at midnight', () => {
    const late = msg(1, { ts: new Date(2026, 10, 16, 23, 59, 50).getTime() });
    const early = msg(2, { ts: late.ts + 20_000 });
    expect(early.ts - late.ts).toBeLessThan(GROUP_GAP_MS);
    expect(sameGroup(late, early)).toBe(false);
  });
});

describe('buildRows', () => {
  it('adds one day separator per day and marks sender group edges', () => {
    const messages = [
      msg(1, { sender: '@a:x' }),
      msg(2, { sender: '@a:x' }),
      msg(3, { sender: '@b:x', senderName: 'Bo' }),
      msg(4, { sender: '@b:x', senderName: 'Bo', ts: NOON + DAY }),
    ];
    const rows = buildRows(messages, { groupChat: true, now: NOON + DAY });
    expect(rows.filter((r) => r.kind === 'day')).toHaveLength(2);

    const list = messageRows(rows);
    expect(list.map((r) => [r.first, r.last])).toEqual([
      [true, false],
      [false, true],
      [true, true],
      [true, true],
    ]);
    expect(list[0]?.showName).toBe(true);
    expect(list[1]?.showName).toBe(false);
  });

  it('never shows a name for own messages or outside group chats', () => {
    const mine = msg(1, { fromMe: true, sender: '@me:x' });
    const theirs = msg(2, { sender: '@a:x' });
    const groupRows = messageRows(buildRows([mine, theirs], { groupChat: true, now: NOON }));
    expect(groupRows.map((r) => r.showName)).toEqual([false, true]);
    const dmRows = messageRows(buildRows([theirs], { groupChat: false, now: NOON }));
    expect(dmRows[0]?.showName).toBe(false);
  });

  it('marks only the last message of the list as newest', () => {
    const rows = messageRows(buildRows([msg(1), msg(2)], { groupChat: false, now: NOON }));
    expect(rows.map((r) => r.newest)).toEqual([false, true]);
  });

  it('reuses the row object while the message and its group flags are unchanged', () => {
    const memo = new Map<string, MessageRow>();
    const a = msg(1);
    const first = messageRows(buildRows([a], { groupChat: false, now: NOON }, memo))[0];
    const again = messageRows(buildRows([a], { groupChat: false, now: NOON }, memo))[0];
    expect(first).toBeDefined();
    expect(again).toBe(first);
    const grown = messageRows(buildRows([a, msg(2)], { groupChat: false, now: NOON }, memo))[0];
    expect(grown).not.toBe(first);
    expect(grown?.newest).toBe(false);
  });

  it('gives a jumbo size to emoji-only rows', () => {
    const rows = messageRows(buildRows([msg(1, { body: '🎉🎉🎉' })], { groupChat: false, now: NOON }));
    expect(rows[0]?.jumbo).toBe(40);
  });
});
