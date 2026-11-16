<script lang="ts">
  import { accounts } from '../../lib/stores/accounts.svelte';
  import { core, type AppPrefs } from '../../lib/core';
  import { settings } from '../../lib/stores/settings.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import ActionRow from './ActionRow.svelte';

  interface Props {
    desktop: boolean;
  }

  let { desktop }: Props = $props();

  const COLOUR = 'var(--cat-chats)';

  let rev = $state(0);
  let note = $state('');
  let synced = $state<Record<string, boolean>>({});

  /** Networks with at least one account, in spectrum order. */
  const networks = $derived([...new Set(accounts.sortedAccounts.map((a) => a.network))]);

  async function save(patch: Partial<AppPrefs>): Promise<void> {
    try {
      await settings.updateApp(patch);
    } catch {
      rev++;
      note = 'Could not save';
    }
  }

  async function sync(network: string): Promise<void> {
    try {
      await core.syncHistory({ network });
      synced = { ...synced, [network]: true };
    } catch {
      note = 'Could not sync';
    }
  }
</script>

{#key rev}
  {#if desktop}
    <Toggle label="Close to tray" color={COLOUR} checked={settings.app.closeToTray} onchange={(on) => void save({ closeToTray: on })} />
    <Toggle
      label="Launch at login"
      color={COLOUR}
      checked={settings.app.launchAtLogin}
      onchange={(on) => void save({ launchAtLogin: on })}
    />
  {/if}
{/key}
{#each networks as network (network)}
  <ActionRow label={`Sync ${accounts.network(network)?.name ?? network}`} value={synced[network] ? 'Done' : ''} onclick={() => void sync(network)} />
{/each}
{#if networks.length === 0}
  <p class="t-body t-muted empty">Connect a network to sync its history.</p>
{/if}
{#if note}<div class="note"><StatusStrip tone="warn" message={note} /></div>{/if}

<style>
  .empty {
    padding: var(--s-2) var(--gutter);
  }

  .note {
    padding: 0 var(--gutter);
  }
</style>
