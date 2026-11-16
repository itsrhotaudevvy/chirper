<script lang="ts">
  import { bannerCss } from '../spectrum';

  interface Props {
    count: number;
    /** Network id: the pill takes its bright banner colour, with banner ink. */
    network?: string;
  }

  let { count, network }: Props = $props();
  const label = $derived(count > 99 ? '99+' : String(count));
  const color = $derived(network ? bannerCss(network) : 'var(--banner-spectrum-7)');
</script>

{#if count > 0}
  <span class="pill t-label" style:--pill={color} aria-label="{count} unread">{label}</span>
{/if}

<style>
  .pill {
    display: inline-grid;
    place-items: center;
    min-width: var(--unread-h);
    height: var(--unread-h);
    padding: 0 var(--s-2);
    border-radius: var(--r-pill);
    background: var(--pill);
    color: var(--on-banner);
    font-variant-numeric: tabular-nums;
    font-weight: 700;
  }
</style>
