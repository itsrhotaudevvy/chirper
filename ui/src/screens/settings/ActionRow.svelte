<script lang="ts">
  import Icon from '../../lib/components/Icon.svelte';

  interface Props {
    label: string;
    /** Short value shown on the trailing side (a count, a version, a word). */
    value?: string;
    tone?: 'default' | 'danger';
    chevron?: boolean;
    disabled?: boolean;
    onclick: () => void;
  }

  let { label, value = '', tone = 'default', chevron = false, disabled = false, onclick }: Props = $props();
</script>

<button class="row" class:danger={tone === 'danger'} type="button" {disabled} onclick={onclick}>
  <span class="label t-headline">{label}</span>
  {#if value}<span class="value t-label">{value}</span>{/if}
  {#if chevron}<Icon name="chevron-right" size={20} />{/if}
</button>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    min-height: var(--hit);
    padding: var(--s-3) var(--gutter);
    text-align: start;
    color: var(--on);
  }

  .row:disabled {
    opacity: 0.45;
  }

  .label {
    flex: 1;
    min-width: 0;
  }

  .row.danger .label {
    color: var(--danger);
  }

  .value {
    color: var(--on-muted);
    font-variant-numeric: tabular-nums;
  }

  .row :global(svg) {
    color: var(--on-muted);
  }
</style>
