<script lang="ts">
  import { onMount } from 'svelte';
  import { core, type AppInfo } from '../../lib/core';
  import { settings } from '../../lib/stores';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import ActionRow from './ActionRow.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import Button from '../../lib/components/Button.svelte';
  import { updater, type UpdaterState } from '../../lib/updater';

  interface Props {
    info: AppInfo | null;
  }

  let { info }: Props = $props();

  const SOURCE = 'https://github.com/itsrhotaudevvy/chirper';
  let note = $state('');
  let updateState = $state<UpdaterState>(updater.state);

  onMount(() => {
    return updater.subscribe((s) => (updateState = s));
  });

  async function openSource(): Promise<void> {
    try {
      await core.openExternal({ url: SOURCE });
    } catch {
      note = 'Could not open the link';
    }
  }

  async function checkUpdates(): Promise<void> {
    note = '';
    await updater.checkForUpdates({ autoDownload: settings.autoUpdate, silent: false });
  }

  async function triggerDownload(): Promise<void> {
    if (updateState.downloadUrl) {
      await updater.download(updateState.downloadUrl, updateState.assetName);
    }
  }
</script>

<div class="info">
  <span class="t-headline">Version</span>
  <span class="t-label">{info?.version ?? '0.1.2'}</span>
</div>
<div class="info">
  <span class="t-headline">Licence</span>
  <span class="t-label">AGPL-3.0-or-later</span>
</div>
<p class="promise t-body">No telemetry. No servers. Your messages stay on this device.</p>

<div class="updates-card">
  <Toggle
    label="Auto-download updates from GitHub"
    color="var(--cat-about)"
    checked={settings.autoUpdate}
    onchange={(on) => settings.updateLocal({ autoUpdate: on })}
  />
  <div class="update-action">
    <Button
      variant="quiet"
      disabled={updateState.status === 'checking' || updateState.status === 'downloading'}
      onclick={() => void checkUpdates()}
    >
      {updateState.status === 'checking'
        ? 'Checking GitHub…'
        : updateState.status === 'downloading'
          ? 'Downloading…'
          : 'Check for updates'}
    </Button>
    {#if updateState.status === 'available' || updateState.status === 'ready'}
      <Button color="accent" onclick={() => void triggerDownload()}>
        {updateState.status === 'ready' ? 'Open installer' : `Download ${updateState.latestVersion}`}
      </Button>
    {/if}
  </div>
  {#if updateState.status === 'checking'}
    <div class="note"><StatusStrip tone="info" message="Checking GitHub for latest release…" /></div>
  {:else if updateState.status === 'up-to-date'}
    <div class="note"><StatusStrip tone="info" message={`Chirper is up to date (v${info?.version ?? '0.1.0'}).`} /></div>
  {:else if updateState.status === 'ready'}
    <div class="note">
      <StatusStrip tone="info" message={`Update ${updateState.latestVersion} downloaded. Run installer to finish.`} />
    </div>
  {:else if updateState.status === 'available'}
    <div class="note">
      <StatusStrip tone="warn" message={`Update ${updateState.latestVersion} available on GitHub.`} />
    </div>
  {:else if updateState.status === 'error'}
    <div class="note"><StatusStrip tone="warn" message={updateState.error ?? 'Could not check updates'} /></div>
  {/if}
</div>

<ActionRow label="Source code on GitHub" chevron onclick={() => void openSource()} />
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

  .updates-card {
    display: grid;
    gap: var(--s-2);
    padding: var(--s-2) 0;
  }

  .update-action {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    padding: 0 var(--gutter);
  }

  .note {
    padding: 0 var(--gutter);
  }
</style>
