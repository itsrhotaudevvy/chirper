<script lang="ts">
  import { onMount } from 'svelte';
  import { accounts, accountKey, accountRing } from '../../lib/stores/accounts.svelte';
  import type { Account } from '../../lib/core';
  import Icon from '../../lib/components/Icon.svelte';
  import ActionRow from './ActionRow.svelte';
  import AccountRing from './AccountRing.svelte';

  interface Props {
    onopen: (account: Account) => void;
    onadd: () => void;
  }

  let { onopen, onadd }: Props = $props();

  onMount(() => {
    void accounts.load();
  });

  /** Up to four accounts by default; more collapse into one row, so the default view stays short. */
  const COLLAPSE_ABOVE = 4;
  let showAll = $state(false);
  const all = $derived(accounts.sortedAccounts);
  const collapsed = $derived(!showAll && all.length > COLLAPSE_ABOVE);
  const shown = $derived(collapsed ? all.slice(0, 3) : all);
</script>

{#each shown as account (accountKey(account))}
  {@const ring = accountRing(account)}
  <button
    class="account"
    type="button"
    aria-label={`${account.name || account.network}, ${account.network}${ring.word ? `, ${ring.word}` : ''}`}
    onclick={() => onopen(account)}
  >
    <AccountRing {account} />
    <span class="name t-headline">{account.name || account.network}</span>
    {#if ring.word}<span class="word t-label" class:warn={ring.tone === 'warn'} class:danger={ring.tone === 'danger'}>{ring.word}</span>{/if}
    <Icon name="chevron-right" size={20} />
  </button>
{/each}

{#if collapsed}
  <ActionRow label={`All accounts (${all.length})`} chevron onclick={() => (showAll = true)} />
{/if}
{#if all.length === 0}
  <p class="empty t-body t-muted">No accounts yet.</p>
{/if}
<ActionRow label="Add network" tone="default" onclick={onadd} />

<style>
  .account {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    min-height: var(--row-h);
    padding: var(--s-2) var(--gutter);
    text-align: start;
    color: var(--on);
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* A status word: plain text, or a small solid pill when it needs attention (banner colours). */
  .word {
    flex: none;
    padding: 0 var(--s-2);
    border-radius: var(--r-pill);
    background: var(--surface-high);
    font-weight: 700;
    color: var(--on);
  }

  .word.warn {
    background: var(--banner-warn);
    color: var(--on-banner);
  }

  .word.danger {
    background: var(--banner-danger);
    color: var(--on-banner);
  }

  .account :global(svg) {
    color: var(--on-muted);
  }

  .empty {
    padding: var(--s-2) var(--gutter);
  }
</style>
