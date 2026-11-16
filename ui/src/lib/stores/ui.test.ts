import { describe, expect, it } from 'vitest';
import { clampInboxWidth, INBOX_WIDTH, layoutFor } from './ui.svelte';

describe('layout', () => {
  it('picks phone, tablet or split from the width', () => {
    expect(layoutFor(390)).toBe('phone');
    expect(layoutFor(700)).toBe('tablet');
    expect(layoutFor(900)).toBe('split');
  });

  it('clamps the inbox width to 300-480 px and falls back to 360 for junk', () => {
    expect(clampInboxWidth(120)).toBe(INBOX_WIDTH.min);
    expect(clampInboxWidth(900)).toBe(INBOX_WIDTH.max);
    expect(clampInboxWidth(401.4)).toBe(401);
    expect(clampInboxWidth('wide')).toBe(INBOX_WIDTH.default);
    expect(clampInboxWidth(Number.NaN)).toBe(INBOX_WIDTH.default);
  });
});
