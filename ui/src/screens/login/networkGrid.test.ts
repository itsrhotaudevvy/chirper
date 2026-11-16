import { describe, expect, it } from 'vitest';
import type { Network } from '../../lib/core';
import { networkTiles } from './networkGrid';

function network(id: string, name = id, available = true): Network {
  return {
    id,
    name,
    color: '',
    beta: false,
    loginFlows: [],
    capabilities: { edits: false, reactions: false, replies: false, redactions: false, typing: false, readReceipts: false, stickers: false, voice: false },
    available,
    reason: available ? '' : 'Offline',
  };
}

describe('networkTiles', () => {
  it('orders tiles in spectrum order, whatever the core returns', () => {
    const tiles = networkTiles([network('messenger'), network('whatsapp'), network('instagram'), network('telegram'), network('discord')]);
    expect(tiles.map((t) => t.id)).toEqual(['instagram', 'whatsapp', 'telegram', 'discord', 'messenger']);
  });

  it('orders networks that share a slot alphabetically by name', () => {
    const tiles = networkTiles([network('zeta', 'Zeta'), network('alpha', 'Alpha')]);
    expect(tiles.map((t) => t.id)).toEqual(['alpha', 'zeta']);
  });

  it('keeps unavailable networks in place, with their reason', () => {
    const tiles = networkTiles([network('gvoice', 'Google Voice', false), network('whatsapp')]);
    expect(tiles.map((t) => [t.id, t.available, t.reason])).toEqual([
      ['gvoice', false, 'Offline'],
      ['whatsapp', true, ''],
    ]);
  });

  it('colours each tile with its network token', () => {
    expect(networkTiles([network('whatsapp')])[0]?.colour).toBe('var(--banner-whatsapp)');
  });
});
