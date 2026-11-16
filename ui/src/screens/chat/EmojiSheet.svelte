<script lang="ts">
  /* A grid of emoji for reactions and the composer. Picking one closes the sheet. */
  import Sheet from '../../lib/components/Sheet.svelte';
  import { EMOJI_GRID } from './emoji';

  interface Props {
    open: boolean;
    title: string;
    onpick: (emoji: string) => void;
    onclose: () => void;
  }

  let { open, title, onpick, onclose }: Props = $props();
</script>

<Sheet {open} {title} {onclose}>
  <div class="grid">
    {#each EMOJI_GRID as emoji (emoji)}
      <button class="cell" type="button" aria-label={emoji} onclick={() => onpick(emoji)}>{emoji}</button>
    {/each}
  </div>
</Sheet>

<style>
  .grid {
    display: grid;
    grid-template-columns: repeat(8, minmax(0, 1fr));
    gap: var(--s-1);
    padding-bottom: var(--s-2);
  }

  .cell {
    display: grid;
    place-items: center;
    aspect-ratio: 1;
    min-height: var(--hit);
    border-radius: var(--r-sm);
    font-size: var(--t-title-size);
    transition: transform var(--dur-quick) var(--ease-out);
  }

  @media (hover: hover) and (pointer: fine) {
    .cell:hover {
      background: var(--surface-high);
    }
  }

  .cell:active {
    transform: scale(0.9);
  }
</style>
