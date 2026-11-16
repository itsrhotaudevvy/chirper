<script lang="ts">
  import { core } from '../../lib/core';
  import { settings } from '../../lib/stores/settings.svelte';
  import type { LoginView } from './loginView';
  import { qrMatrix } from './qr';

  interface Props {
    view: Extract<LoginView, { kind: 'qr' | 'code' }>;
    /** The network's bright banner colour: the code sits on it, as a banner. */
    colour: string;
  }

  let { view, colour }: Props = $props();

  const matrix = $derived(view.kind === 'qr' ? matrixOf(view.data) : null);

  function matrixOf(data: string): ReturnType<typeof qrMatrix> | null {
    try {
      return qrMatrix(data);
    } catch {
      return null;
    }
  }

  /* A QR code is a login secret while it is on screen: block screenshots and recording for as long as
     it is visible, then restore the user's own setting. */
  $effect(() => {
    if (view.kind !== 'qr') return;
    void core.setSecureScreen({ enabled: true }).catch(() => undefined);
    return () => {
      void core.setSecureScreen({ enabled: settings.app.secureScreen }).catch(() => undefined);
    };
  });
</script>

{#if view.kind === 'qr'}
  {#if matrix}
    <div class="qr" role="img" aria-label="QR code to scan with your phone">
      <svg viewBox="-2 -2 {matrix.size + 4} {matrix.size + 4}" shape-rendering="crispEdges" aria-hidden="true">
        <path d={matrix.d} fill="#000000" />
      </svg>
    </div>
  {:else}
    <p class="t-body">This code is too large to show.</p>
  {/if}
{:else}
  <p class="huge t-display" aria-live="polite" style:--code={colour}>{view.text}</p>
{/if}

<style>
  .qr {
    justify-self: center;
    width: min(78vw, 300px);
    padding: var(--s-4);
    border-radius: var(--r-lg);
    background: #ffffff;
    box-shadow: var(--elev-1);
  }

  .qr svg {
    width: 100%;
    height: auto;
  }

  /* The code is a banner: a solid bright block with banner ink, the network colour. */
  .huge {
    justify-self: stretch;
    padding: var(--s-6) var(--gutter);
    border-radius: var(--banner-radius);
    background: var(--code);
    color: var(--on-banner);
    letter-spacing: 0.12em;
    font-variant-numeric: tabular-nums;
    text-align: center;
    overflow-wrap: anywhere;
  }
</style>
