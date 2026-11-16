import { describe, expect, it } from 'vitest';
import { qrMatrix } from './qr';

describe('qrMatrix', () => {
  it('uses version 1 (21 modules) for short text at error correction M', () => {
    expect(qrMatrix('hello').size).toBe(21);
  });

  it('draws finder patterns: the top-left module is dark and the path is not empty', () => {
    const { size, d } = qrMatrix('2@login-token');
    expect(d.startsWith('M0 0h1v1h-1z')).toBe(true);
    expect(d.length).toBeGreaterThan(size);
  });

  it('is deterministic for the same text', () => {
    expect(qrMatrix('same')).toEqual(qrMatrix('same'));
  });
});
