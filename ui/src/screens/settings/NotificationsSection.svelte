<script lang="ts">
  import { onMount } from 'svelte';
  import { core, type AppPrefs, type Network } from '../../lib/core';
  import { settings } from '../../lib/stores/settings.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';

  let rev = $state(0);
  let note = $state('');
  let networks = $state<Network[]>([]);

  onMount(() => {
    core
      .networks()
      .then((list) => (networks = list.filter((n) => n.available)))
      .catch(() => (networks = []));
  });

  /** Saves app prefs; a failed save is undone by the store and the toggles are redrawn. */
  async function save(patch: Partial<AppPrefs>): Promise<void> {
    try {
      await settings.updateApp(patch);
    } catch {
      rev++;
      note = 'Could not save';
    }
  }

  /** Notifications of a network are on unless the network is in the muted list. */
  function setNetworkOn(id: string, on: boolean): void {
    const muted = settings.app.mutedNetworks.filter((m) => m !== id);
    void save({ mutedNetworks: on ? muted : [...muted, id] });
  }
</script>

{#key rev}
  <Toggle
    label="Notifications"
    color="var(--cat-notifications)"
    checked={settings.app.notifications}
    onchange={(on) => void save({ notifications: on })}
  />
  <Toggle
    label="Hide message text"
    description="Show only the chat name"
    color="var(--cat-notifications)"
    disabled={!settings.app.notifications}
    checked={settings.app.hideNotificationContent}
    onchange={(on) => void save({ hideNotificationContent: on })}
  />
  {#each networks as network (network.id)}
    <Toggle
      label={network.name}
      description="Notifications for {network.name} messages"
      color={network.color}
      disabled={!settings.app.notifications}
      checked={!settings.app.mutedNetworks.includes(network.id)}
      onchange={(on) => setNetworkOn(network.id, on)}
    />
  {/each}
{/key}
{#if note}<div class="note"><StatusStrip tone="warn" message={note} /></div>{/if}

<style>
  .note {
    padding: 0 var(--gutter);
  }
</style>
