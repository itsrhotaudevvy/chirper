<script lang="ts">
  import { settings } from '../../lib/stores/settings.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import type { AppPrefs } from '../../lib/core';

  let rev = $state(0);
  let note = $state('');

  /** Saves app prefs; a failed save is undone by the store and the toggles are redrawn. */
  async function save(patch: Partial<AppPrefs>): Promise<void> {
    try {
      await settings.updateApp(patch);
    } catch {
      rev++;
      note = 'Could not save';
    }
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
{/key}
{#if note}<div class="note"><StatusStrip tone="warn" message={note} /></div>{/if}

<style>
  .note {
    padding: 0 var(--gutter);
  }
</style>
