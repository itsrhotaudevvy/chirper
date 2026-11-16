<script lang="ts">
  /* Image, video or sticker. The box keeps the media's aspect ratio, so the list never jumps when it loads. */
  import type { MessageMedia } from '../../lib/core';
  import { mediaUrl } from '../../lib/media';
  import Skeleton from '../../lib/components/Skeleton.svelte';

  interface Props {
    media: MessageMedia;
    kind: 'image' | 'video' | 'sticker';
    /** Opens the full-screen viewer with the source (null when the core has no file for it). */
    onopen: (src: string | null, kind: 'image' | 'video') => void;
  }

  let { media, kind, onopen }: Props = $props();

  const MAX_WIDTH = 280;
  const MAX_HEIGHT = 360;
  const src = $derived(mediaUrl(media.url));
  const ratio = $derived(media.width > 0 && media.height > 0 ? media.width / media.height : 4 / 3);
  /* Width follows the ratio, capped so a tall picture stays within MAX_HEIGHT. */
  const width = $derived(kind === 'sticker' ? 140 : Math.min(MAX_WIDTH, Math.round(MAX_HEIGHT * ratio)));
  const viewable = $derived(kind !== 'sticker');
</script>

{#if viewable}
  <button
    class="tile"
    type="button"
    style:width="{width}px"
    style:aspect-ratio={String(ratio)}
    aria-label={kind === 'video' ? 'Play video' : 'Open photo'}
    onclick={() => onopen(src, kind === 'video' ? 'video' : 'image')}
  >
    {#if src && kind === 'video'}
      <video src={src} preload="metadata" muted playsinline></video>
      <span class="play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M8 5.5v13l10.5-6.5z" /></svg></span>
    {:else if src}
      <img src={src} alt="" loading="lazy" decoding="async" />
    {:else}
      <span class="placeholder"><Skeleton width="100%" height="100%" radius="0" /></span>
    {/if}
  </button>
{:else if src}
  <img class="sticker" src={src} alt="" loading="lazy" decoding="async" style:width="{width}px" />
{:else}
  <span class="sticker-missing" style:width="{width}px" aria-hidden="true"></span>
{/if}

<style>
  .tile {
    position: relative;
    display: block;
    max-width: 100%;
    overflow: hidden;
    border-radius: inherit;
    background: var(--surface-high);
  }

  .tile img,
  .tile video {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .placeholder {
    position: absolute;
    inset: 0;
  }

  .play {
    position: absolute;
    left: 50%;
    top: 50%;
    display: grid;
    place-items: center;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    background: rgb(255 255 255 / 0.92);
    transform: translate(-50%, -50%);
  }

  .play svg {
    width: 20px;
    height: 20px;
    fill: var(--on-banner);
  }

  .sticker {
    height: auto;
  }

  .sticker-missing {
    display: block;
    aspect-ratio: 1;
    border-radius: var(--r-md);
    background: var(--surface-high);
  }
</style>
