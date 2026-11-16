<script lang="ts">
  import { core, type AppPrefs } from '../../lib/core';
  import { settings } from '../../lib/stores/settings.svelte';
  import { haptic } from '../../lib/haptics';
  import Segmented from '../../lib/components/Segmented.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';

  const COLOUR = 'var(--cat-privacy)';
  const LOCK_AFTER = [
    { value: '0', label: 'Instant' },
    { value: '60', label: '1 min' },
    { value: '300', label: '5 min' },
    { value: '900', label: '15 min' },
  ];

  let rev = $state(0);
  let note = $state('');

  async function save(patch: Partial<AppPrefs>): Promise<void> {
    try {
      await settings.updateApp(patch);
    } catch {
      rev++;
      note = 'Could not save';
    }
  }

  /**
   * Turning the lock on first asks the OS for authentication, so a lock that cannot be opened is
   * never enabled. If that fails, the toggle goes back.
   */
  async function setLock(on: boolean): Promise<void> {
    if (on) {
      try {
        const result = await core.unlock();
        if (!result.unlocked) throw new Error('not unlocked');
      } catch {
        rev++;
        note = 'Lock is not available here';
        haptic('warning');
        return;
      }
    }
    await save({ appLock: on });
  }
</script>

{#key rev}
  <Toggle label="App lock" color={COLOUR} checked={settings.app.appLock} onchange={(on) => void setLock(on)} />
  {#if settings.app.appLock}
    <div class="choice">
      <span class="label t-label">Lock after</span>
      <Segmented
        label="Lock after"
        options={LOCK_AFTER}
        value={String(settings.app.lockAfterSecs)}
        onchange={(next) => void save({ lockAfterSecs: Number(next) })}
      />
    </div>
  {/if}
  <Toggle
    label="Block screenshots"
    color={COLOUR}
    checked={settings.app.secureScreen}
    onchange={(on) => void save({ secureScreen: on })}
  />
  <Toggle label="Haptics" color={COLOUR} checked={settings.app.haptics} onchange={(on) => void save({ haptics: on })} />
{/key}
{#if note}<div class="note"><StatusStrip tone="warn" message={note} /></div>{/if}

<style>
  .choice {
    display: grid;
    gap: var(--s-2);
    padding: 0 var(--gutter);
  }

  .label {
    color: var(--on-muted);
  }

  .note {
    padding: 0 var(--gutter);
  }
</style>
