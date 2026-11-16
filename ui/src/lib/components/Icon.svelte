<script lang="ts">
  import { ICONS, type IconName, type IconSize } from './icons';

  interface Props {
    name: IconName;
    /** Pixel size of the square icon. */
    size?: IconSize;
    /** Accessible name. Omit for decorative icons (they are hidden from screen readers). */
    label?: string;
  }

  let { name, size = 24, label }: Props = $props();
  const paths = $derived(ICONS[name]);
</script>

<svg
  class="icon"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.75"
  stroke-linecap="round"
  stroke-linejoin="round"
  role={label ? 'img' : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : 'true'}
  focusable="false"
>
  {#each paths as d (d)}
    <path {d} />
  {/each}
</svg>

<style>
  .icon {
    flex: none;
    display: block;
  }
</style>
