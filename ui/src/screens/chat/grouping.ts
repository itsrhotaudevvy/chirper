/**
 * Pure timeline logic for the chat screen: sender groups, day separators, the geometry flags a row
 * needs (DESIGN 4, messages) and jumbo-emoji detection. No DOM, no state: unit-tested in
 * grouping.test.ts.
 */
import type { Message } from '../../lib/core';

/** Messages from one sender closer than this (ms) form one group. */
export const GROUP_GAP_MS = 5 * 60 * 1000;

export type TimelineRow =
  | { kind: 'top'; key: string }
  | { kind: 'day'; key: string; label: string }
  | MessageRow;

export interface MessageRow {
  kind: 'msg';
  key: string;
  message: Message;
  /** First message of its sender group (gets the 12 px gap above it). */
  first: boolean;
  /** Last message of its sender group (shows time and ticks). */
  last: boolean;
  /** Group chats: the sender name is drawn above the first message of an incoming group. */
  showName: boolean;
  /** The newest message of the room. */
  newest: boolean;
  /** Set when the message is a jumbo emoji row; the font size in px. */
  jumbo: number | null;
}

const EXTENDED_PICTO = /\p{Extended_Pictographic}|\p{Regional_Indicator}/u;
const SEGMENTER = typeof Intl !== 'undefined' && 'Segmenter' in Intl ? new Intl.Segmenter() : null;

/**
 * Counts the emoji in a body that is nothing but emoji (spaces aside). Returns 0 for anything else,
 * so text with a single letter never turns jumbo.
 */
export function emojiOnlyCount(body: string): number {
  const text = body.trim();
  if (!text || text.length > 64) return 0;
  const graphemes = SEGMENTER
    ? [...SEGMENTER.segment(text)].map((s) => s.segment)
    : [...text];
  const visible = graphemes.filter((g) => g.trim() !== '');
  if (visible.length === 0 || visible.length > 3) return 0;
  return visible.every((g) => EXTENDED_PICTO.test(g)) ? visible.length : 0;
}

/** Jumbo font size for an emoji-only text message: 52 px for one or two, 40 px for three. */
export function jumboSize(message: Pick<Message, 'type' | 'body' | 'media' | 'redacted' | 'replyTo'>): number | null {
  if (message.type !== 'text' || message.redacted || message.replyTo || message.media) return null;
  const count = emojiOnlyCount(message.body);
  if (count === 0) return null;
  return count === 3 ? 40 : 52;
}

/** Local calendar day as a sortable key, for comparing two timestamps. */
export function dayKey(ts: number): string {
  const d = new Date(ts);
  return `${d.getFullYear()}-${d.getMonth() + 1}-${d.getDate()}`;
}

/** "Today", "Yesterday", a weekday within the last week, or a date. */
export function dayLabel(ts: number, now: number): string {
  const today = dayKey(now);
  if (dayKey(ts) === today) return 'Today';
  const yesterday = new Date(now);
  yesterday.setDate(yesterday.getDate() - 1);
  if (dayKey(ts) === dayKey(yesterday.getTime())) return 'Yesterday';
  const age = (now - ts) / 86_400_000;
  const sameYear = new Date(ts).getFullYear() === new Date(now).getFullYear();
  if (age >= 0 && age < 7) return new Intl.DateTimeFormat(undefined, { weekday: 'long' }).format(ts);
  return new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'short', year: sameYear ? undefined : 'numeric' }).format(ts);
}

/** Local "HH:MM" time for a message. */
export function timeLabel(ts: number): string {
  return new Intl.DateTimeFormat(undefined, { hour: '2-digit', minute: '2-digit' }).format(ts);
}

/** True when two consecutive messages belong to the same sender group (same day, same sender, close in time). */
export function sameGroup(prev: Message | undefined, cur: Message): boolean {
  if (!prev) return false;
  return prev.sender === cur.sender && dayKey(prev.ts) === dayKey(cur.ts) && cur.ts - prev.ts <= GROUP_GAP_MS;
}

/**
 * Turns the ordered messages into timeline rows: a day separator before each new day, and the
 * group flags of each message. `memo` (keyed by eventId) returns the previous row object when nothing
 * the row shows has changed, so unchanged rows keep their identity across updates.
 */
export function buildRows(
  messages: readonly Message[],
  options: { groupChat: boolean; now: number },
  memo: Map<string, MessageRow> = new Map(),
): TimelineRow[] {
  const out: TimelineRow[] = [];
  const last = messages.length - 1;
  let previousDay = '';
  messages.forEach((message, i) => {
    const prev = messages[i - 1];
    const next = messages[i + 1];
    const day = dayKey(message.ts);
    if (day !== previousDay) {
      out.push({ kind: 'day', key: `day-${day}`, label: dayLabel(message.ts, options.now) });
      previousDay = day;
    }
    const first = !sameGroup(prev, message);
    const isLast = next === undefined || !sameGroup(message, next);
    const showName = options.groupChat && !message.fromMe && first;
    const newest = i === last;
    out.push(rowFor(message, { first, last: isLast, showName, newest }, memo));
  });
  return out;
}

interface RowFlags {
  first: boolean;
  last: boolean;
  showName: boolean;
  newest: boolean;
}

function rowFor(message: Message, flags: RowFlags, memo: Map<string, MessageRow>): MessageRow {
  const cached = memo.get(message.eventId);
  if (cached && cached.message === message && sameFlags(cached, flags)) return cached;
  const row: MessageRow = {
    kind: 'msg',
    key: message.eventId,
    message,
    first: flags.first,
    last: flags.last,
    showName: flags.showName,
    newest: flags.newest,
    jumbo: jumboSize(message),
  };
  memo.set(message.eventId, row);
  return row;
}

function sameFlags(row: MessageRow, flags: RowFlags): boolean {
  return row.first === flags.first && row.last === flags.last && row.showName === flags.showName && row.newest === flags.newest;
}
