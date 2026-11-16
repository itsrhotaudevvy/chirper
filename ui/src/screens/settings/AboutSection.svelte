<script lang="ts">
  import { core, type AppInfo } from '../../lib/core';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import ActionRow from './ActionRow.svelte';

  interface Props {
    info: AppInfo | null;
  }

  let { info }: Props = $props();

  const SOURCE = 'https://github.com/chirper-app/chirper';
  let note = $state('');

  async function openSource(): Promise<void> {
    try {
      await core.openExternal({ url: SOURCE });
    } catch {
      note = 'Could not open the link';
    }
  }
</script>

<div class="info">
  <span class="t-headline">Version</span>
  <span class="t-label">{info?.version ?? '...'}</span>
</div>
<div class="info">
  <span class="t-headline">Licence</span>
  <span class="t-label">AGPL-3.0-or-later</span>
</div>
<p class="promise t-body">No telemetry. No servers. Your messages stay on this device.</p>
<ActionRow label="Source code" chevron onclick={() => void openSource()} />
{#if note}<div class="note"><StatusStrip tone="warn" message={note} /></div>{/if}

<style>
  .info {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    min-height: var(--hit);
    padding: var(--s-3) var(--gutter);
  }

  .info .t-label {
    color: var(--on-muted);
    font-variant-numeric: tabular-nums;
  }

  .promise {
    padding: var(--s-2) var(--gutter);
    font-weight: 600;
  }

  .note {
    padding: 0 var(--gutter);
  }
</style>
