import { describe, expect, it } from 'vitest';
import { createSyncBundle, restoreSyncBundle } from './sync';

describe('sync bundle', () => {
  it('encrypts and restores sync bundle round-trip', async () => {
    const pin = '482910';
    const bundle = await createSyncBundle(pin);
    expect(bundle.startsWith('CHIRPER_SYNC_V1:')).toBe(true);

    const restored = await restoreSyncBundle(bundle, pin);
    expect(restored).toBeDefined();
    expect(restored.accountsCount).toBeGreaterThanOrEqual(0);
  });

  it('rejects decryption with incorrect PIN', async () => {
    const bundle = await createSyncBundle('correct-pin');
    await expect(restoreSyncBundle(bundle, 'wrong-pin')).rejects.toThrow();
  });
});
