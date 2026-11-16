<script lang="ts">
  import { onMount } from 'svelte';
  import { core, type Account, type CoreSettings } from '../../lib/core';
  import { accounts, accountRing } from '../../lib/stores/accounts.svelte';
  import { bannerCss } from '../../lib/spectrum';
  import { router } from '../../lib/router.svelte';
  import Banner from '../../lib/components/Banner.svelte';
  import Button from '../../lib/components/Button.svelte';
  import Sheet from '../../lib/components/Sheet.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import ActionRow from './ActionRow.svelte';
  import { flagFor, withFlag } from './coreFlags';

  interface Props {
    account: Account;
    onback: () => void;
  }

  let { account, onback }: Props = $props();

  const network = $derived(accounts.network(account.network));
  const ring = $derived(accountRing(account));
  const colour = $derived(bannerCss(account.network));

  let values = $state<CoreSettings | null>(null);
  let rev = $state(0);
  let confirming = $state(false);
  let more = $state(false);
  let note = $state('');

  onMount(() => {
    core
      .settings()
      .then((s) => (values = s))
      .catch(() => (note = 'Could not load settings'));
  });

  /** Saves one per-network flag. The change shows at once; a failed save is undone. */
  async function setFlag(key: 'sendReadReceipts' | 'sendTyping', value: boolean): Promise<void> {
    const current = values;
    if (!current) return;
    const next: CoreSettings = { ...current, [key]: withFlag(current[key], account.network, value) };
    values = next;
    try {
      await core.setSettings({ settings: next });
    } catch {
      values = current;
      rev++;
      note = 'Could not save';
    }
  }

  async function logout(): Promise<void> {
    try {
      await core.logout({ network: account.network, loginId: account.loginId });
      confirming = false;
      onback();
    } catch {
      confirming = false;
      note = 'Could not log out';
    }
  }

  async function sync(): Promise<void> {
    try {
      await core.syncHistory({ network: account.network });
      note = 'Syncing history';
    } catch {
      note = 'Could not sync';
    }
  }
</script>

<Banner
  colors={[colour]}
  title={account.name || account.network}
  status={ring.word || network?.name || ''}
  action={{ label: 'Back', onclick: onback }}
/>

<div class="column">
  {#if ring.tone === 'warn' || ring.tone === 'danger'}
    <ActionRow label="Reconnect" tone="default" chevron onclick={() => router.go('add-network', { network: account.network })} />
  {/if}

  {#if values && network?.capabilities.readReceipts}
    {#key rev}
      <Toggle
        label="Send read receipts"
        color={colour}
        checked={flagFor(values.sendReadReceipts, account.network)}
        onchange={(on) => void setFlag('sendReadReceipts', on)}
      />
    {/key}
  {/if}
  {#if values && network?.capabilities.typing}
    {#key rev}
      <Toggle
        label="Send typing"
        color={colour}
        checked={flagFor(values.sendTyping, account.network)}
        onchange={(on) => void setFlag('sendTyping', on)}
      />
    {/key}
  {/if}

  <ActionRow label={more ? 'Less' : 'More'} onclick={() => (more = !more)} />
  {#if more}
    <ActionRow label="Sync history" onclick={() => void sync()} />
  {/if}

  <ActionRow label="Log out" tone="danger" onclick={() => (confirming = true)} />

  {#if note}<StatusStrip tone="warn" message={note} />{/if}
</div>

<Sheet open={confirming} title={`Log out of ${account.name || account.network}?`} onclose={() => (confirming = false)}>
  <div class="confirm">
    <p class="t-body t-muted">Chats stay on this device until you remove them.</p>
    <Button color="var(--danger)" onclick={() => void logout()}>Log out</Button>
    <Button variant="quiet" onclick={() => (confirming = false)}>Cancel</Button>
  </div>
</Sheet>

<style>
  .column {
    display: grid;
    gap: var(--s-1);
    max-width: 640px;
    margin: 0 auto;
    padding: var(--s-4) var(--gutter) calc(var(--safe-bottom) + var(--s-8));
  }

  .confirm {
    display: grid;
    gap: var(--s-3);
    padding-top: var(--s-2);
  }
</style>
