<script lang="ts" generics="T extends string">
  import { haptic } from '../haptics';

  interface Option {
    value: T;
    label: string;
  }

  interface Props {
    options: readonly Option[];
    value: T;
    /** Required: names the group for screen readers. */
    label: string;
    onchange?: (value: T) => void;
  }

  let { options, value = $bindable(), label, onchange }: Props = $props();

  function choose(next: T): void {
    if (next === value) return;
    value = next;
    haptic('selection');
    onchange?.(next);
  }

  function onKeydown(event: KeyboardEvent, index: number): void {
    if (event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') return;
    event.preventDefault();
    const step = event.key === 'ArrowRight' ? 1 : -1;
    const next = options[(index + step + options.length) % options.length];
    if (next) choose(next.value);
  }
</script>

<div class="segmented" role="radiogroup" aria-label={label}>
  {#each options as option, i (option.value)}
    {@const selected = option.value === value}
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      tabindex={selected ? 0 : -1}
      class="segment"
      class:selected
      onclick={() => choose(option.value)}
      onkeydown={(e) => onKeydown(e, i)}
    >
      {option.label}
    </button>
  {/each}
</div>

<style>
  .segmented {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: var(--r-pill);
    background: var(--surface-high);
  }

  .segment {
    flex: 1 1 0;
    min-height: var(--hit);
    padding: 0 var(--s-3);
    border-radius: var(--r-pill);
    color: var(--on-muted);
    font-weight: 600;
    transition: background-color var(--dur-base) var(--ease-out), color var(--dur-base) var(--ease-out);
  }

  .segment.selected {
    background: var(--surface);
    color: var(--on);
    box-shadow: var(--elev-1);
  }
</style>
