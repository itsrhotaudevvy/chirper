// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync } from 'svelte';
import { formatHash, parseDeepLink, parseHash, router } from './router.svelte';
import { ui } from './stores/ui.svelte';

describe('hash routes', () => {
  it('parses every view and falls back to the inbox', () => {
    expect(parseHash('#/')).toEqual({ view: 'inbox' });
    expect(parseHash('')).toEqual({ view: 'inbox' });
    expect(parseHash('#/settings')).toEqual({ view: 'settings' });
    expect(parseHash('#/add-network/telegram')).toEqual({ view: 'add-network', network: 'telegram' });
    expect(parseHash('#/add-network')).toEqual({ view: 'add-network' });
    expect(parseHash('#/nonsense/x')).toEqual({ view: 'inbox' });
    expect(parseHash('#/chat')).toEqual({ view: 'inbox' });
  });

  it('formats routes and round-trips a room id with special characters', () => {
    const roomId = '!abc/def:chirper.local';
    const hash = formatHash({ view: 'chat', roomId });
    expect(hash).toBe('#/chat/!abc%2Fdef%3Achirper.local');
    expect(parseHash(hash)).toEqual({ view: 'chat', roomId });
    expect(formatHash({ view: 'inbox' })).toBe('#/');
    expect(formatHash({ view: 'add-network', network: 'slack' })).toBe('#/add-network/slack');
  });

  it('does not throw on a malformed percent escape', () => {
    expect(parseHash('#/chat/%E0%A4%A')).toEqual({ view: 'chat', roomId: '%E0%A4%A' });
  });
});

describe('deep links', () => {
  it('maps chirper://chat/<roomId> to a chat route', () => {
    expect(parseDeepLink('chirper://chat/!room:chirper.local')).toEqual({ view: 'chat', roomId: '!room:chirper.local' });
    expect(parseDeepLink('chirper://chat/%21room%3Achirper.local?x=1')).toEqual({
      view: 'chat',
      roomId: '!room:chirper.local',
    });
  });

  it('ignores links the app does not handle', () => {
    expect(parseDeepLink('https://example.org/chat/x')).toBeNull();
    expect(parseDeepLink('chirper://settings')).toBeNull();
    expect(parseDeepLink('chirper://chat/')).toBeNull();
  });
});

describe('navigation', () => {
  afterEach(() => {
    delete (document as { startViewTransition?: unknown }).startViewTransition;
    ui.width = 1024;
  });

  it('go pushes a history entry, back returns to the previous route', () => {
    ui.width = 390;
    router.replace('inbox');
    router.go('chat', { roomId: 'r1' });
    expect(router.view).toBe('chat');
    expect(window.location.hash).toBe('#/chat/r1');
    router.back();
    expect(router.route).toEqual({ view: 'inbox' });
  });

  it('animates phone moves with the View Transitions API when it exists', () => {
    ui.width = 390;
    const start = vi.fn((update: () => Promise<void>) => {
      void update();
      return { finished: Promise.resolve() };
    });
    Object.assign(document, { startViewTransition: start });
    router.replace('inbox');
    router.go('settings');
    expect(start).toHaveBeenCalledTimes(1);
    expect(document.documentElement.dataset['nav']).toBe('forward');
    expect(router.view).toBe('settings');
  });

  it('does not animate in split view', () => {
    ui.width = 1200;
    const start = vi.fn();
    Object.assign(document, { startViewTransition: start });
    router.replace('inbox');
    router.go('settings');
    expect(start).not.toHaveBeenCalled();
    expect(router.view).toBe('settings');
  });

  it('replace swaps the route without adding a history entry', () => {
    ui.width = 1200;
    router.replace('inbox');
    const before = window.history.length;
    router.replace('chat', { roomId: 'r2' });
    flushSync();
    expect(window.history.length).toBe(before);
    expect(ui.selectedRoomId).toBe('r2');
  });
});
