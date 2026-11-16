import { describe, expect, it } from 'vitest';
import { flagFor, withFlag } from './coreFlags';

describe('flagFor', () => {
  it('uses the network value, then the "*" default, then the fallback', () => {
    expect(flagFor({ whatsapp: false, '*': true }, 'whatsapp')).toBe(false);
    expect(flagFor({ whatsapp: false, '*': true }, 'telegram')).toBe(true);
    expect(flagFor({}, 'telegram')).toBe(true);
    expect(flagFor({}, 'telegram', false)).toBe(false);
  });
});

describe('withFlag', () => {
  it('sets one network and leaves the others and the default alone', () => {
    const before = { '*': true, signal: true };
    expect(withFlag(before, 'whatsapp', false)).toEqual({ '*': true, signal: true, whatsapp: false });
    expect(before).toEqual({ '*': true, signal: true });
  });
});
