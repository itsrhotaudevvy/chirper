<script lang="ts">
  import { onMount } from 'svelte';
  import { core } from '../../lib/core';
  import { haptic } from '../../lib/haptics';
  import BrandHero from '../../lib/components/BrandHero.svelte';
  import Button from '../../lib/components/Button.svelte';

  /** Lock surface. Fully opaque: nothing of the app shows through it. */
  let busy = $state(false);
  let failed = $state(false);

  async function unlock(): Promise<void> {
    if (busy) return;
    busy = true;
    try {
      const result = await core.unlock();
      failed = !result.unlocked;
      if (failed) haptic('warning');
    } catch {
      failed = true;
      haptic('warning');
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    void unlock();
  });
</script>

<div class="lock" role="dialog" aria-modal="true" aria-labelledby="lock-title">
  <BrandHero id="lock-title" short />

  <div class="actions">
    {#if failed}
      <p class="state t-label" role="alert">Try again</p>
    {/if}
    <Button onclick={() => void unlock()} disabled={busy}>Unlock</Button>
  </div>
</div>

<style>
  .lock {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg);
    color: var(--on);
  }

  .actions {
    display: flex;
    flex: 1;
    flex-direction: column;
    justify-content: flex-start;
    align-items: flex-start;
    gap: var(--s-4);
    padding: var(--s-8) calc(var(--gutter) + var(--safe-right)) calc(var(--safe-bottom) + var(--s-8))
      calc(var(--gutter) + var(--safe-left));
  }

  .state {
    padding: var(--s-1) var(--s-3);
    border-radius: var(--r-pill);
    background: var(--banner-warn);
    color: var(--on-banner);
    font-weight: 700;
  }
</style>
