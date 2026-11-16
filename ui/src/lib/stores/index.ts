import { core } from '../core';
import { accounts } from './accounts.svelte';
import { chats } from './chats.svelte';
import { messages } from './messages.svelte';
import { settings } from './settings.svelte';

export { accounts } from './accounts.svelte';
export { chats } from './chats.svelte';
export { messages, PAGE_SIZE } from './messages.svelte';
export { settings, type LocalPrefs, type ThemePref } from './settings.svelte';
export { ui, layoutFor, type LayoutMode } from './ui.svelte';

/**
 * Loads the initial state and routes every core-batch to the stores. Returns a disposer.
 * Call once at start-up.
 */
export async function connectStores(): Promise<() => void> {
  await core.start();
  await Promise.all([accounts.load(), chats.load(), settings.loadApp()]);
  return core.onEvents((events) => {
    chats.apply(events);
    messages.apply(events);
    accounts.apply(events);
  });
}
