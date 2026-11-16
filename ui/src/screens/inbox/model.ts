/**
 * Pure inbox logic: which chats show, in which order, grouped how, and how a row's time reads.
 * Kept free of Svelte so it is unit-tested directly (model.test.ts).
 */
import type { Chat } from '../../lib/core';
import { sortBySpectrum } from '../../lib/spectrum';

/** Chats of one network (or all networks when `network` is null) whose name contains `query`. */
export function visibleChats(chats: readonly Chat[], network: string | null, query: string): Chat[] {
  const needle = query.trim().toLocaleLowerCase();
  return chats.filter(
    (chat) =>
      !chat.isSpace &&
      (network === null || chat.network === network) &&
      (needle === '' || chat.name.toLocaleLowerCase().includes(needle)),
  );
}

/** Distinct networks of the chats, in spectrum order. */
export function networksOf(chats: readonly Chat[]): string[] {
  const unique = [...new Set(chats.map((c) => c.network))];
  return sortBySpectrum(unique, (n) => n);
}

export interface NetworkGroup {
  network: string;
  chats: Chat[];
}

/** Groups chats by network, groups in spectrum order. Each group keeps the order it had. */
export function groupByNetwork(chats: readonly Chat[]): NetworkGroup[] {
  const byNetwork = new Map<string, Chat[]>();
  for (const chat of chats) {
    const list = byNetwork.get(chat.network);
    if (list) list.push(chat);
    else byNetwork.set(chat.network, [chat]);
  }
  return networksOf(chats).map((network) => ({ network, chats: byNetwork.get(network) ?? [] }));
}

export function unreadOf(chats: readonly Chat[]): number {
  return chats.reduce((sum, c) => sum + c.unreadCount, 0);
}

/** Pinned chats of a list, in their list order (used by ⌘/Ctrl+1..9). */
export function pinnedOf(chats: readonly Chat[]): Chat[] {
  return chats.filter((c) => c.pinned);
}

/** Actions on a chat, from the row menu or a swipe. */
export type ChatAction = 'pin' | 'mute' | 'archive' | 'read';

/** "Muted until" for an indefinite mute (year 9999, far beyond any clock in use). */
export const MUTE_FOREVER = 253402300799000;

/** A row of the list, or the colour band that opens a network group. */
export type ListItem =
  | { key: string; kind: 'band'; network: string; label: string }
  | { key: string; kind: 'row'; chat: Chat };

const DAY_MS = 86_400_000;

/**
 * Short time for a row: clock time today, a weekday within the last six days, otherwise a date.
 * `now` is passed in so the function is deterministic.
 */
export function formatRowTime(ts: number, now: number): string {
  if (ts <= 0) return '';
  const when = new Date(ts);
  const today = new Date(now);
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  if (ts >= startOfToday) return when.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
  if (ts >= startOfToday - 6 * DAY_MS) return when.toLocaleDateString(undefined, { weekday: 'short' });
  return when.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
}

/** Preview line: groups show the sender's first name before the body. */
export function previewText(chat: Chat): string {
  const body = chat.preview?.body ?? '';
  if (!chat.isGroup || !chat.preview?.sender || !body) return body;
  const first = chat.preview.sender.split(/\s+/)[0] ?? chat.preview.sender;
  return `${first}: ${body}`;
}

/** Index of the nearest row to `current` after moving by `step` (clamped to the list). */
export function moveIndex(count: number, current: number, step: number): number {
  if (count === 0) return -1;
  return Math.min(count - 1, Math.max(0, current + step));
}
