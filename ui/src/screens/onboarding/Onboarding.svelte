<script lang="ts">
  import { core } from '../../lib/core';
  import { accounts } from '../../lib/stores';
  import { router } from '../../lib/router.svelte';
  import BrandHero from '../../lib/components/BrandHero.svelte';
  import Button from '../../lib/components/Button.svelte';

  interface Props {
    ondone: () => void;
  }

  let { ondone }: Props = $props();

  let busy = $state(false);
  let failed = $state(false);

  /** Turns on the demo network in the core, then opens its login. */
  async function tryDemo(): Promise<void> {
    busy = true;
    failed = false;
    try {
      const settings = await core.settings();
      await core.setSettings({ settings: { ...settings, demoNetwork: true } });
      await accounts.load();
      router.go('add-network', { network: 'demo' });
      ondone();
    } catch {
      failed = true;
    } finally {
      busy = false;
    }
  }

  function addNetwork(): void {
    router.go('add-network');
    ondone();
  }
</script>

<section class="onboard" aria-labelledby="onboard-title">
  <BrandHero id="onboard-title">
    <p class="sentence t-title">All your chats. One place. Nothing leaves your device.</p>
  </BrandHero>

  <div class="actions">
    <Button onclick={addNetwork} disabled={busy}>Add a network</Button>
    <button class="link t-headline" type="button" disabled={busy} onclick={() => void tryDemo()}>
      {busy ? 'Starting the demo' : 'Try the demo'}
    </button>
    {#if failed}<p class="failed t-label" role="alert">Failed</p>{/if}
  </div>
</section>

<style>
  .onboard {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg);
    color: var(--on);
  }

  .sentence {
    max-width: 24ch;
    margin: 0;
    font-weight: 650;
  }

  .actions {
    display: flex;
    flex: 1;
    flex-direction: column;
    justify-content: flex-end;
    align-items: flex-start;
    gap: var(--s-3);
    padding: var(--s-6) calc(var(--gutter) + var(--safe-right)) calc(var(--safe-bottom) + var(--s-8))
      calc(var(--gutter) + var(--safe-left));
  }

  /* A text link, not a chip: underlined, on the page. */
  .link {
    min-height: var(--hit);
    padding: 0;
    color: var(--on);
    font-weight: 600;
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .link:disabled {
    opacity: 0.5;
  }

  .failed {
    padding: var(--s-1) var(--s-3);
    border-radius: var(--r-pill);
    background: var(--banner-danger);
    color: var(--on-banner);
    font-weight: 700;
  }
</style>
