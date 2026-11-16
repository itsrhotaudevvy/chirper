<script lang="ts">
  import { networkCss } from '../../lib/spectrum';

  interface Props {
    /** Networks with chats, in spectrum order. */
    networks: string[];
    /** Selected network id, or null for all. */
    selected: string | null;
    /** Display name of a network, for the accessible label. */
    nameOf: (network: string) => string;
    /** CSS of the "All" disc: the rainbow of every network in the inbox. */
    allFill: string;
    onselect: (network: string | null) => void;
  }

  let { networks, selected, nameOf, allFill, onselect }: Props = $props();
</script>

<div class="strip" role="group" aria-label="Filter by network">
  <button class="chip" type="button" aria-label="All networks" aria-pressed={selected === null} onclick={() => onselect(null)}>
    <span class="disc" style:--fill={allFill}></span>
  </button>
  {#each networks as network (network)}
    <button
      class="chip"
      type="button"
      aria-label={nameOf(network)}
      aria-pressed={selected === network}
      title={nameOf(network)}
      onclick={() => onselect(network)}
    >
      <span class="disc" style:--fill={networkCss(network)}></span>
    </button>
  {/each}
</div>

<style>
  /*
   * Solid discs in spectrum order (DESIGN 3.3). Ten items (All + nine networks) fit a 390 px phone
   * with the gutters; more scroll sideways, and the edges fade into the gutter.
   */
  .strip {
    display: flex;
    align-items: center;
    min-height: var(--hit);
    padding: 0 var(--gutter);
    overflow-x: auto;
    scrollbar-width: none;
    -webkit-mask-image: linear-gradient(to right, transparent 0, #000 var(--gutter), #000 calc(100% - var(--gutter)), transparent 100%);
    mask-image: linear-gradient(to right, transparent 0, #000 var(--gutter), #000 calc(100% - var(--gutter)), transparent 100%);
  }

  .strip::-webkit-scrollbar {
    display: none;
  }

  /* 34 px wide, 44 px tall; the hit area reaches 5 px further on each side, so it is 44 px square. */
  .chip {
    position: relative;
    flex: none;
    display: grid;
    place-items: center;
    width: 34px;
    height: var(--hit);
    border-radius: var(--r-pill);
  }

  .chip::after {
    content: '';
    position: absolute;
    inset: 0 -5px;
  }

  .disc {
    width: 24px;
    height: 24px;
    border-radius: 50%;
    background: var(--fill);
    transition: box-shadow var(--dur-base) var(--ease-out);
  }

  /* Selected: a 3 px --on ring around the disc, with a 2 px gap of page colour. */
  .chip[aria-pressed='true'] .disc {
    box-shadow: 0 0 0 2px var(--bg), 0 0 0 5px var(--on);
  }
</style>
