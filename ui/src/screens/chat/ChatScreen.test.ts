// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ChatScreen from './ChatScreen.svelte';
import EmptyChat from './EmptyChat.svelte';
import { connectStores } from '../../lib/stores';

/*
 * Mounts the chat screen against the demo core (FakeCore in happy-dom). Covers the header, the
 * timeline and the composer; the grouping rules themselves are tested in grouping.test.ts.
 */

const FAMILY = '!whatsapp.family:chirper.local';

let instance: ReturnType<typeof mount> | undefined;
let host: HTMLElement | undefined;
let dispose: (() => void) | undefined;

afterEach(() => {
  if (instance) unmount(instance);
  dispose?.();
  host?.remove();
  instance = undefined;
  host = undefined;
  dispose = undefined;
});

describe('ChatScreen', () => {
  it('renders the banner title, the timeline and a composer for a loaded chat', async () => {
    dispose = await connectStores();
    host = document.createElement('div');
    document.body.appendChild(host);
    expect(() => {
      instance = mount(ChatScreen, { target: host!, props: { roomId: FAMILY, onback: () => undefined } });
      flushSync();
    }).not.toThrow();

    await vi.waitFor(() => {
      expect(host?.querySelectorAll('[role="listitem"]').length ?? 0).toBeGreaterThan(0);
    }, { timeout: 4000 });

    expect(host.querySelector('.hero h1')?.textContent).toContain('Family');
    expect(host.querySelector('textarea[aria-label="Message"]')).not.toBeNull();
    // happy-dom is 1024 px wide: split view, where the inbox is the way back and no button is shown.
    expect(host.querySelector('[aria-label="Back"]')).toBeNull();
  });

  it('has no voice button, and a send button that waits for text', async () => {
    dispose = await connectStores();
    host = document.createElement('div');
    document.body.appendChild(host);
    instance = mount(ChatScreen, { target: host, props: { roomId: FAMILY, onback: () => undefined } });
    flushSync();
    await vi.waitFor(() => expect(host?.querySelector('[role="listitem"]')).not.toBeNull(), { timeout: 4000 });

    expect(host.querySelector('[aria-label*="Voice"]')).toBeNull();
    const send = host.querySelector<HTMLButtonElement>('button[aria-label="Send"]');
    expect(send?.disabled).toBe(true);

    const field = host.querySelector<HTMLTextAreaElement>('textarea[aria-label="Message"]');
    if (!field) throw new Error('no composer field');
    field.value = 'hello';
    field.dispatchEvent(new Event('input'));
    flushSync();
    expect(host.querySelector<HTMLButtonElement>('button[aria-label="Send"]')?.disabled).toBe(false);
  });

  it('shows the empty placeholder of the split view as one line', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    instance = mount(EmptyChat, { target: host });
    flushSync();
    expect(host.textContent?.trim()).toBe('Pick a chat');
  });
});
