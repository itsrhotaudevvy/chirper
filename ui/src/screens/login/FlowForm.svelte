<script lang="ts">
  import { untrack } from 'svelte';
  import Button from '../../lib/components/Button.svelte';
  import Segmented from '../../lib/components/Segmented.svelte';
  import TextField from '../../lib/components/TextField.svelte';
  import { fieldErrors, type FieldView } from './loginView';

  interface Props {
    fields: readonly FieldView[];
    /** Network id: the Continue button takes its colour. */
    network: string;
    busy: boolean;
    onsubmit: (values: Record<string, string>) => void;
  }

  let { fields, network, busy, onsubmit }: Props = $props();

  /* Every field starts empty: TextField binds to a value with a fallback, which must not be undefined. */
  let values = $state<Record<string, string>>(
    untrack(() => Object.fromEntries(fields.map((field) => [field.id, ''] as const))),
  );
  let tried = $state(false);
  const errors = $derived(tried ? fieldErrors(fields, values) : {});

  function submit(): void {
    tried = true;
    if (Object.keys(fieldErrors(fields, values)).length > 0 || busy) return;
    onsubmit({ ...values });
  }
</script>

<form
  class="form"
  onsubmit={(event) => {
    event.preventDefault();
    submit();
  }}
>
  {#each fields as field (field.id)}
    {#if field.options.length > 0}
      <div class="choice">
        <span class="label t-label">{field.label}</span>
        <Segmented
          label={field.label}
          options={field.options.map((o) => ({ value: o, label: o }))}
          value={values[field.id] ?? ''}
          onchange={(next) => (values[field.id] = next)}
        />
      </div>
    {:else}
      <TextField
        label={field.label}
        type={field.input}
        inputmode={field.inputmode}
        autocomplete={field.autocomplete}
        bind:value={values[field.id]}
        hint={errors[field.id]}
        invalid={errors[field.id] !== undefined}
        onsubmit={submit}
      />
    {/if}
  {/each}
  <div class="actions">
    <Button type="submit" color={network} disabled={busy}>Continue</Button>
  </div>
</form>

<style>
  .form {
    display: grid;
    gap: var(--s-4);
  }

  .choice {
    display: grid;
    gap: var(--s-2);
  }

  .label {
    color: var(--on-muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
