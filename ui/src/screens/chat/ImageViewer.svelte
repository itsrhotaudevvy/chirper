<script lang="ts">
  /*
   * Full-screen media viewer. Escape or a drag down closes it. Video plays with its controls.
   * Without a file (the browser demo) it shows a placeholder.
   */
  import { trapFocus } from '../../lib/a11y';
  import IconButton from '../../lib/components/IconButton.svelte';

  interface Props {
    src: string | null;
    kind: 'image' | 'video';
    onclose: () => void;
  }

  let { src, kind, onclose }: Props = $props();

  const DISMISS_PX = 120;
  let drag = $state(0);
  let startY = 0;
  let dragging = false;

  function down(event: PointerEvent): void {
    if (kind === 'video') return;
    dragging = true;
    startY = event.clientY;
  }

  function move(event: PointerEvent): void {
    if (dragging) drag = Math.max(0, event.clientY - startY);
  }

  function up(): void {
    if (!dragging) return;
    dragging = false;
    if (drag > DISMISS_PX) onclose();
    drag = 0;
  }
</script>

<div
  class="viewer"
  role="dialog"
  tabindex="-1"
  aria-modal="true"
  aria-label={kind === 'video' ? 'Video' : 'Photo'}
  use:trapFocus={{ onEscape: onclose }}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={up}
  style:--drag="{drag}px"
>
  <div class="close">
    <IconButton icon="close" label="Close" onclick={onclose} color="#fff" />
  </div>

  <div class="stage" style:transform="translateY(var(--drag))" style:opacity={drag > 0 ? 1 - drag / 400 : 1}>
    {#if src && kind === 'video'}
      <!-- svelte-ignore a11y_media_has_caption -->
      <video {src} controls playsinline autoplay></video>
    {:else if src}
      <img {src} alt="" decoding="async" />
    {:else}
      <p class="placeholder t-body">Not available in this preview</p>
    {/if}
  </div>
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgb(0 0 0 / 0.92);
    color: #fff;
    animation: fade var(--dur-base) var(--ease-out);
  }

  .close {
    position: absolute;
    top: calc(var(--s-2) + var(--safe-top));
    right: calc(var(--s-2) + var(--safe-right));
    z-index: 1;
  }

  .stage {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    padding: calc(var(--s-12) + var(--safe-top)) var(--gutter) calc(var(--s-4) + var(--safe-bottom));
    touch-action: pan-x;
    will-change: transform;
  }

  .stage img,
  .stage video {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: var(--r-md);
  }

  .placeholder {
    color: rgb(255 255 255 / 0.7);
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }
</style>
