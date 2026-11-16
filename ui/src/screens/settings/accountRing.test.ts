import { describe, expect, it } from 'vitest';
import type { Account, AccountState, Network } from '../../lib/core';
import { accountRing, AccountsStore } from '../../lib/stores/accounts.svelte';

function account(network: string, state: AccountState, name = network): Account {
  return { network, loginId: `${network}-1`, name, avatarUrl: '', state, error: '', message: '' };
}

function network(id: string, name = id): Network {
  return {
    id,
    name,
    color: '',
    beta: false,
    loginFlows: [],
    capabilities: { edits: false, reactions: false, replies: false, redactions: false, typing: false, readReceipts: false, stickers: false, voice: false },
    available: true,
    reason: '',
  };
}

describe('accountRing', () => {
  it('is solid in the network colour when connected, with no word', () => {
    expect(accountRing(account('whatsapp', 'CONNECTED'))).toEqual({ tone: 'solid', colour: 'var(--net-whatsapp)', word: '' });
  });

  it('pulses in the network colour while connecting', () => {
    expect(accountRing(account('telegram', 'CONNECTING'))).toEqual({ tone: 'pulse', colour: 'var(--net-telegram)', word: 'Connecting' });
  });

  it('uses --warn for accounts that need attention', () => {
    expect(accountRing(account('signal', 'BAD_CREDENTIALS'))).toEqual({ tone: 'warn', colour: 'var(--warn)', word: 'Reconnect' });
    expect(accountRing(account('signal', 'TRANSIENT_DISCONNECT')).tone).toBe('warn');
  });

  it('uses --danger for errors', () => {
    expect(accountRing(account('discord', 'UNKNOWN_ERROR'))).toEqual({ tone: 'danger', colour: 'var(--danger)', word: 'Reconnect' });
  });
});

describe('AccountsStore ordering', () => {
  it('lists networks and accounts in spectrum order', () => {
    const store = new AccountsStore({} as never);
    store.networks = [network('messenger'), network('whatsapp'), network('bluesky'), network('demo')];
    expect(store.sortedNetworks.map((n) => n.id)).toEqual(['whatsapp', 'bluesky', 'demo', 'messenger']);
  });
});
