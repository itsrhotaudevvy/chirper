/**
 * Pure inbox logic: which chats show, in which order, grouped how, and how a row's time reads.
 * Kept free of Svelte so it is unit-tested directly (model.test.ts).
 */
import type { Chat } from '../../lib/core';
import { sortBySpectrum } from '../../lib/spectrum';

export type InboxTab = 'chats' | 'servers';

export function isServerChannel(chat: Chat): boolean {
  return !chat.isSpace && (Boolean(chat.isChannel) || Boolean(chat.parentId && chat.parentId !== ''));
}

export function isDirectChat(chat: Chat): boolean {
  return !chat.isSpace && !isServerChannel(chat);
}

/** Chats of one network (or all networks when `network` is null) whose name contains `query`.
 * When `tab` is 'chats', server channels are filtered out.
 * When `tab` is 'servers', only server channels are kept.
 */
export function visibleChats(
  chats: readonly Chat[],
  network: string | null,
  query: string,
  tab: InboxTab = 'chats',
): Chat[] {
  const needle = query.trim().toLocaleLowerCase();
  return chats.filter((chat) => {
    if (chat.isSpace) return false;
    if (tab === 'chats' && isServerChannel(chat)) return false;
    if (tab === 'servers' && !isServerChannel(chat)) return false;
    if (network !== null && chat.network !== network) return false;
    if (needle !== '') {
      const matchName = chat.name.toLocaleLowerCase().includes(needle);
      const matchServer = chat.serverName && chat.serverName.toLocaleLowerCase().includes(needle);
      if (!matchName && !matchServer) return false;
    }
    return true;
  });
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

export interface ServerGroup {
  serverKey: string;
  serverName: string;
  network: string;
  chats: Chat[];
}

/** Groups server channels by their parent server. */
export function groupByServer(chats: readonly Chat[]): ServerGroup[] {
  const byServer = new Map<string, { serverName: string; network: string; chats: Chat[] }>();
  for (const chat of chats) {
    const key = chat.parentId || chat.serverName || chat.network;
    const name =
      chat.serverName ||
      (chat.network ? `${chat.network.charAt(0).toUpperCase() + chat.network.slice(1)} Server` : 'Server');
    const entry = byServer.get(key);
    if (entry) {
      entry.chats.push(chat);
    } else {
      byServer.set(key, { serverName: name, network: chat.network, chats: [chat] });
    }
  }
  return [...byServer.entries()].map(([serverKey, data]) => ({
    serverKey,
    serverName: data.serverName,
    network: data.network,
    chats: data.chats,
  }));
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

/** A row of the list, or the colour band that opens a network or server group. */
export type ListItem =
  | { key: string; kind: 'band'; network: string; label: string }
  | { key: string; kind: 'server'; serverKey: string; network: string; label: string }
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
