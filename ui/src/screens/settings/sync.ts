import { core, type Account, type AppPrefs, type CoreSettings } from '../../lib/core';
import { settings, type LocalPrefs } from '../../lib/stores/settings.svelte';

export interface SyncPayload {
  v: 1;
  app: 'Chirper';
  exportedAt: string;
  localPrefs: LocalPrefs;
  appPrefs: AppPrefs;
  coreSettings: CoreSettings;
  accounts: Array<Pick<Account, 'network' | 'loginId' | 'name' | 'avatarUrl'>>;
  chats: Array<{ roomId: string; pinned?: boolean; archived?: boolean; mutedUntil?: number }>;
}

export interface EncryptedSyncEnvelope {
  v: 1;
  type: 'chirper-sync-bundle';
  salt: string;
  iv: string;
  ciphertext: string;
}

const SYNC_PREFIX = 'CHIRPER_SYNC_V1:';

function bytesToHex(bytes: Uint8Array): string {
  let hex = '';
  for (let i = 0; i < bytes.length; i++) {
    const b = bytes[i];
    if (b !== undefined) hex += b.toString(16).padStart(2, '0');
  }
  return hex;
}

function hexToBytes(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

async function deriveKey(passphrase: string, salt: Uint8Array, usage: 'encrypt' | 'decrypt'): Promise<CryptoKey> {
  const enc = new TextEncoder();
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    enc.encode(passphrase.trim()),
    { name: 'PBKDF2' },
    false,
    ['deriveKey'],
  );
  return crypto.subtle.deriveKey(
    {
      name: 'PBKDF2',
      salt: salt as unknown as BufferSource,
      iterations: 100_000,
      hash: 'SHA-256',
    },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    [usage],
  );
}

/**
 * Creates an encrypted cross-platform sync bundle protected by the user's passphrase or PIN.
 */
export async function createSyncBundle(passphrase: string): Promise<string> {
  if (!passphrase.trim()) {
    throw new Error('Please enter a passphrase or PIN to encrypt your sync bundle.');
  }

  const [coreSettings, rawAccounts, chatList] = await Promise.all([
    core.settings().catch((): CoreSettings => ({ demoNetwork: false, sendReadReceipts: {}, sendTyping: {}, telegram: { apiId: 0, apiHash: '' } })),
    core.accounts().catch(() => []),
    core.listChats({ filter: { network: '', archived: false, unreadOnly: false, query: '' } }).catch(() => []),
  ]);

  const payload: SyncPayload = {
    v: 1,
    app: 'Chirper',
    exportedAt: new Date().toISOString(),
    localPrefs: {
      theme: settings.theme,
      accent: settings.accent,
      reduceMotion: settings.reduceMotion,
    },
    appPrefs: { ...settings.app },
    coreSettings,
    accounts: rawAccounts.map((a) => ({
      network: a.network,
      loginId: a.loginId,
      name: a.name,
      avatarUrl: a.avatarUrl,
    })),
    chats: chatList
      .filter((c) => c.pinned || c.archived || (c.mutedUntil ?? 0) > 0)
      .map((c) => ({
        roomId: c.roomId,
        pinned: c.pinned,
        archived: c.archived,
        mutedUntil: c.mutedUntil,
      })),
  };

  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(passphrase, salt, 'encrypt');

  const enc = new TextEncoder();
  const plaintext = enc.encode(JSON.stringify(payload));

  const cipherBuffer = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, plaintext);
  const ciphertext = bytesToHex(new Uint8Array(cipherBuffer));

  const envelope: EncryptedSyncEnvelope = {
    v: 1,
    type: 'chirper-sync-bundle',
    salt: bytesToHex(salt),
    iv: bytesToHex(iv),
    ciphertext,
  };

  const jsonStr = JSON.stringify(envelope);
  const b64 = btoa(jsonStr);
  return `${SYNC_PREFIX}${b64}`;
}

/**
 * Decrypts and restores an encrypted sync bundle onto this device.
 */
export async function restoreSyncBundle(
  rawInput: string,
  passphrase: string,
): Promise<{ accountsCount: number; chatsCount: number }> {
  if (!passphrase.trim()) {
    throw new Error('Please enter the passphrase or PIN used when exporting.');
  }

  let cleaned = rawInput.trim();
  if (cleaned.startsWith(SYNC_PREFIX)) {
    cleaned = cleaned.slice(SYNC_PREFIX.length).trim();
  }

  let envelope: EncryptedSyncEnvelope;
  try {
    const jsonStr = atob(cleaned);
    envelope = JSON.parse(jsonStr) as EncryptedSyncEnvelope;
  } catch {
    try {
      envelope = JSON.parse(cleaned) as EncryptedSyncEnvelope;
    } catch {
      throw new Error('Invalid sync bundle format. Please check the code or file.');
    }
  }

  if (envelope.type !== 'chirper-sync-bundle' || !envelope.salt || !envelope.iv || !envelope.ciphertext) {
    throw new Error('Unrecognized sync bundle format.');
  }

  const salt = hexToBytes(envelope.salt);
  const iv = hexToBytes(envelope.iv);
  const cipherBytes = hexToBytes(envelope.ciphertext);

  let key: CryptoKey;
  try {
    key = await deriveKey(passphrase, salt, 'decrypt');
  } catch (err: unknown) {
    throw new Error(`Key derivation error: ${err instanceof Error ? err.message : String(err)}`);
  }

  let decryptedBuffer: ArrayBuffer;
  try {
    decryptedBuffer = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: iv as unknown as BufferSource },
      key,
      cipherBytes as unknown as BufferSource,
    );
  } catch {
    throw new Error('Incorrect passphrase or PIN. Decryption failed.');
  }

  const dec = new TextDecoder();
  const payloadStr = dec.decode(decryptedBuffer);
  const payload = JSON.parse(payloadStr) as SyncPayload;

  if (payload.app !== 'Chirper' || payload.v !== 1) {
    throw new Error('Bundle is not a valid Chirper sync package.');
  }

  // 1. Apply local preferences (theme, accent, reduceMotion)
  if (payload.localPrefs) {
    settings.updateLocal(payload.localPrefs);
  }

  // 2. Apply app preferences (notifications, lock, haptics, etc.)
  if (payload.appPrefs) {
    await settings.updateApp(payload.appPrefs).catch(() => undefined);
  }

  // 3. Apply core engine settings
  if (payload.coreSettings) {
    await core.setSettings({ settings: payload.coreSettings }).catch(() => undefined);
  }

  // 4. Restore chat organization flags
  let chatsRestored = 0;
  if (payload.chats && Array.isArray(payload.chats)) {
    for (const chat of payload.chats) {
      if (!chat.roomId) continue;
      const flags: { pinned?: boolean; archived?: boolean; mutedUntil?: number } = {};
      if (chat.pinned !== undefined) flags.pinned = chat.pinned;
      if (chat.archived !== undefined) flags.archived = chat.archived;
      if (chat.mutedUntil !== undefined) flags.mutedUntil = chat.mutedUntil;
      await core.setChatFlags({ roomId: chat.roomId, flags }).catch(() => undefined);
      chatsRestored++;
    }
  }

  return {
    accountsCount: payload.accounts?.length ?? 0,
    chatsCount: chatsRestored,
  };
}
