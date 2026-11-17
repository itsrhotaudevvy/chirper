<script lang="ts">
  import { mediaUrl } from '../media';
  import { networkCss, personHue } from '../spectrum';

  export type AvatarSize = 28 | 36 | 48 | 88;

  interface Props {
    /** Display name, used for initials and the accessible name. */
    name: string;
    /** Stable id of the person or room; decides the background hue. Defaults to the name. */
    id?: string | undefined;
    /** Avatar image as a Matrix content URI (mxc://...). Other URLs are not loaded. */
    src?: string | undefined;
    /** Network id: draws a 2 px ring in its spectrum colour. */
    network?: string | undefined;
    size?: AvatarSize | undefined;
  }

  let { name, id, src, network, size = 48 }: Props = $props();

  const url = $derived(src ? mediaUrl(src) : null);
  let failed = $state(false);
  $effect(() => {
    if (url) failed = false;
  });
  const hue = $derived(personHue(id ?? name));
  const initials = $derived(
    name
      .trim()
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((w) => w[0]?.toUpperCase() ?? '')
      .join('') || '?',
  );
  /** The network colour of the ring (a page mark, so the theme column). */
  const ringColour = $derived(network ? networkCss(network) : 'transparent');
  /** Initials use the type scale: small avatars take the label size, large ones the title size. */
  const fontSize = $derived(size <= 28 ? 'var(--t-label-size)' : size >= 88 ? 'var(--t-title-size)' : 'var(--t-headline-size)');
</script>

<span
  class="avatar"
  role="img"
  aria-label={name}
  style:--size="{size}px"
  style:--hue={hue}
  style:--ring-c={ringColour}
  style:--font={fontSize}
>
  {#if url && !failed}
    <img src={url} alt="" loading="lazy" decoding="async" onerror={() => {
      failed = true;
      console.warn('[avatar] image did not load', url);
    }} />
  {:else}
    <span class="initials" aria-hidden="true">{initials}</span>
  {/if}
</span>

<style>
  .avatar {
    position: relative;
    display: inline-grid;
    place-items: center;
    width: var(--size);
    height: var(--size);
    flex: none;
    border-radius: 50%;
    background: oklch(0.52 0.12 var(--hue));
    color: #fff;
    font-size: var(--font);
    font-weight: 600;
    overflow: visible;
  }

  /*
   * Network ring: 2 px (the --ring token) just outside the avatar, so the person's colour stays
   * readable. The colour is its own variable (--ring-c); --ring stays a length.
   */
  .avatar::before {
    content: '';
    position: absolute;
    inset: calc(var(--ring) * -2);
    border-radius: 50%;
    border: var(--ring) solid var(--ring-c);
    pointer-events: none;
  }

  .avatar img {
    width: 100%;
    height: 100%;
    border-radius: 50%;
    object-fit: cover;
  }

  .initials {
    letter-spacing: -0.02em;
  }
</style>
