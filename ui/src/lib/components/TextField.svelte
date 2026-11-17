<script lang="ts">
  import type { HTMLInputAttributes } from 'svelte/elements';

  interface Props {
    /** Required: visible label above the field and its accessible name. */
    label: string;
    value?: string | undefined;
    type?: 'text' | 'password' | 'email' | 'tel' | 'url' | 'search' | undefined;
    placeholder?: string | undefined;
    autocomplete?: HTMLInputAttributes['autocomplete'] | undefined;
    inputmode?: HTMLInputAttributes['inputmode'] | undefined;
    /** One line of help or error text under the field. */
    hint?: string | undefined;
    /** Marks the hint as an error (warn colour, announced). */
    invalid?: boolean | undefined;
    disabled?: boolean | undefined;
    onsubmit?: () => void | undefined;
  }

  let {
    label,
    value = $bindable(''),
    type = 'text',
    placeholder,
    autocomplete,
    inputmode,
    hint,
    invalid = false,
    disabled = false,
    onsubmit,
  }: Props = $props();

  const hintId = `hint-${Math.random().toString(36).slice(2, 9)}`;

  function onkeydown(event: KeyboardEvent): void {
    if (event.key === 'Enter' && onsubmit) {
      event.preventDefault();
      onsubmit();
    }
  }
</script>

<div class="field">
  <label class="label t-label" for={hintId}>{label}</label>
  <input
    id={hintId}
    class="input t-body"
    class:invalid
    {type}
    {placeholder}
    {autocomplete}
    {inputmode}
    {disabled}
    bind:value
    onkeydown={onkeydown}
    aria-invalid={invalid}
    aria-describedby={hint ? `${hintId}-hint` : undefined}
  />
  {#if hint}
    <p id="{hintId}-hint" class="hint t-label" class:invalid role={invalid ? 'alert' : undefined}>{hint}</p>
  {/if}
</div>

<style>
  .field {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-2);
  }

  .label {
    color: var(--on-muted);
  }

  .input {
    min-height: var(--hit);
    padding: 0 var(--s-4);
    border-radius: var(--r-md);
    background: var(--surface-high);
    color: var(--on);
    border: 1px solid transparent;
    transition: border-color var(--dur-base) var(--ease-out);
  }

  .input:focus-visible {
    outline-offset: 0;
  }

  .input.invalid {
    border-color: var(--danger);
  }

  .input:disabled {
    opacity: 0.5;
  }

  .hint {
    color: var(--on-muted);
  }

  .hint.invalid {
    color: var(--danger);
  }
</style>
