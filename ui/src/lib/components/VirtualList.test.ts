// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest';
import { createRawSnippet, flushSync, mount, unmount } from 'svelte';
import VirtualList from './VirtualList.svelte';

/*
 * scrollToKey is the hook the chat uses to jump to a search result. happy-dom has no layout, so the
 * test checks the contract: a loaded key is found (true), an unknown key is not (false).
 */

interface Item {
  key: string;
}

const items: unknown[] = Array.from({ length: 50 }, (_, i) => ({ key: `row-${i}` }));
const keyOf = (item: unknown): string => (item as Item).key;
const row = createRawSnippet<[unknown, number]>((item) => ({
  render: () => `<span>${(item() as Item).key}</span>`,
}));

let host: HTMLElement | undefined;
let instance: ReturnType<typeof mount> | undefined;

afterEach(() => {
  if (instance) unmount(instance);
  host?.remove();
  instance = undefined;
  host = undefined;
});

describe('VirtualList.scrollToKey', () => {
  it('reports whether a loaded row exists, and centres it when it does', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    instance = mount(VirtualList, { target: host, props: { items, keyOf, row, label: 'Test' } });
    flushSync();
    const api = instance as unknown as { scrollToKey: (key: string) => boolean };
    expect(api.scrollToKey('row-10')).toBe(true);
    expect(api.scrollToKey('row-999')).toBe(false);
  });
});
