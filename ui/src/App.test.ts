// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import App from './App.svelte';
import Gallery from './dev/Gallery.svelte';

/*
 * Mounts the real gallery. A regression such as a state write during render (state_unsafe_mutation)
 * throws here, so it fails `pnpm test` instead of showing a blank page in the webview.
 */

let instance: ReturnType<typeof mount> | undefined;
let host: HTMLElement | undefined;

afterEach(() => {
  if (instance) unmount(instance);
  host?.remove();
  instance = undefined;
  host = undefined;
});

describe('App shell', () => {
  it('mounts without errors', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    expect(() => {
      instance = mount(App, { target: host! });
      flushSync();
    }).not.toThrow();
  });
});

describe('Component gallery', () => {
  it('mounts and renders the banner, chats and timeline without errors', async () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    expect(() => {
      instance = mount(Gallery, { target: host! });
      flushSync();
    }).not.toThrow();

    expect(host.querySelector('h1')?.textContent).toContain('Components');
    // Chats and messages load asynchronously from the demo core.
    await vi.waitFor(() => {
      expect(host?.querySelectorAll('.chat-row').length ?? 0).toBeGreaterThan(0);
      expect(host?.querySelectorAll('[aria-label="Family messages"] .row').length ?? 0).toBeGreaterThan(0);
    }, { timeout: 4000 });
  });
});
