import type { Account, CoreApi, CoreEvent, Network } from '../core';
import { core as defaultCore } from '../core';
import { networkCss, sortBySpectrum } from '../spectrum';
import { SvelteMap } from 'svelte/reactivity';

/** Networks and logged-in accounts. Account state drives the status ring and banner states. */
export class AccountsStore {
  networks = $state.raw<Network[]>([]);
  readonly #accounts = new SvelteMap<string, Account>();

  constructor(private readonly api: CoreApi = defaultCore) {}

  readonly list = $derived([...this.#accounts.values()]);

  /** Every account, in spectrum order (network slot, then name). */
  readonly sortedAccounts = $derived(sortBySpectrum(this.list, (a) => a.network, (a) => a.name));

  /** Every network, in spectrum order (slot, then name). */
  readonly sortedNetworks = $derived(sortBySpectrum(this.networks, (n) => n.id, (n) => n.name));

  /** Accounts of one network, in insertion order. */
  forNetwork(network: string): Account[] {
    return this.list.filter((a) => a.network === network);
  }

  network(id: string): Network | undefined {
    return this.networks.find((n) => n.id === id);
  }

  async load(): Promise<void> {
    const [networks, accounts] = await Promise.all([this.api.networks(), this.api.accounts()]);
    this.networks = networks;
    this.#accounts.clear();
    for (const account of accounts) this.#accounts.set(key(account), account);
  }

  apply(events: readonly CoreEvent[]): void {
    for (const event of events) {
      if (event.kind !== 'account') continue;
      if (event.payload.state === 'LOGGED_OUT') this.#accounts.delete(key(event.payload));
      else this.#accounts.set(key(event.payload), event.payload);
    }
  }
}

function key(account: Pick<Account, 'network' | 'loginId'>): string {
  return `${account.network}/${account.loginId}`;
}

/** Stable key of one account, for lookups in the UI. */
export function accountKey(account: Pick<Account, 'network' | 'loginId'>): string {
  return key(account);
}

/** How an account's status ring looks (DESIGN 0, state = colour). */
export type RingTone = 'solid' | 'pulse' | 'warn' | 'danger';

export interface AccountRing {
  tone: RingTone;
  /** CSS colour of the ring: the network colour, or a banner state colour. */
  colour: string;
  /** One word, only when the account is not connected. Empty when connected. */
  word: string;
}

/**
 * Maps an account state to its ring. Connected = solid network colour; connecting = the same colour
 * pulsing; transient or bad credentials = --warn (needs attention); anything else = --danger.
 */
export function accountRing(account: Pick<Account, 'network' | 'state'>): AccountRing {
  const colour = networkCss(account.network);
  switch (account.state) {
    case 'CONNECTED':
      return { tone: 'solid', colour, word: '' };
    case 'CONNECTING':
      return { tone: 'pulse', colour, word: 'Connecting' };
    case 'TRANSIENT_DISCONNECT':
      return { tone: 'warn', colour: 'var(--warn)', word: 'Offline' };
    case 'BAD_CREDENTIALS':
      return { tone: 'warn', colour: 'var(--warn)', word: 'Reconnect' };
    default:
      return { tone: 'danger', colour: 'var(--danger)', word: 'Reconnect' };
  }
}

export const accounts = new AccountsStore();
