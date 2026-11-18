<script lang="ts">
  import { onMount } from 'svelte';
  import { core, type CoreSettings } from '../../lib/core';
  import Button from '../../lib/components/Button.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import TextField from '../../lib/components/TextField.svelte';

  const COLOUR = 'var(--cat-about)';
  const HASH = /^[0-9a-f]{32}$/i;

  let current = $state<CoreSettings | null>(null);
  let apiId = $state('');
  let apiHash = $state('');
  let note = $state('');
  let warn = $state(false);
  let busy = $state(false);

  onMount(() => {
    core
      .settings()
      .then((s) => {
        current = s;
        apiId = s.telegram.apiId ? String(s.telegram.apiId) : '';
        apiHash = s.telegram.apiHash;
      })
      .catch(() => {
        note = 'Could not load the settings';
        warn = true;
      });
  });

  const idText = $derived(apiId.trim());
  const idOk = $derived(idText === '' || (/^\d+$/.test(idText) && Number(idText) > 0));
  const hashOk = $derived(apiHash.trim() === '' || HASH.test(apiHash.trim()));

  async function save(): Promise<void> {
    if (!current || !idOk || !hashOk) return;
    busy = true;
    try {
      const telegram = { apiId: idText === '' ? 0 : Number(idText), apiHash: apiHash.trim() };
      await core.setSettings({ settings: { ...current, telegram } });
      current = { ...current, telegram };
      note = 'Saved. Restart Chirper before Telegram uses the new values.';
      warn = false;
    } catch {
      note = 'Could not save';
      warn = true;
    } finally {
      busy = false;
    }
  }
</script>

<p class="help t-label">
  Telegram needs your own api_id and api_hash. Create them at my.telegram.org, under API development tools.
</p>
<div class="fields">
  <TextField
    label="API ID"
    inputmode="numeric"
    placeholder="12345678"
    bind:value={apiId}
    invalid={!idOk}
    hint={idOk ? '' : 'A positive number'}
  />
  <TextField
    label="API hash"
    type="password"
    autocomplete="off"
    placeholder="32 letters and digits"
    bind:value={apiHash}
    invalid={!hashOk}
    hint={hashOk ? '' : 'Exactly 32 hexadecimal characters'}
  />
  <div class="actions">
    <Button color={COLOUR} disabled={busy || !current || !idOk || !hashOk} onclick={() => void save()}>Save</Button>
  </div>
</div>
{#if note}<div class="note"><StatusStrip tone={warn ? 'warn' : 'info'} message={note} /></div>{/if}

<style>
  .help,
  .fields,
  .note {
    padding: 0 var(--gutter);
  }

  .help {
    color: var(--on-muted);
    margin: 0 0 var(--s-2);
  }

  .fields {
    display: grid;
    gap: var(--s-3);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
