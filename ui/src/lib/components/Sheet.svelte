<script lang="ts">
  import type { Snippet } from 'svelte';
  import { trapFocus } from '../a11y';

  interface Props {
    open: boolean;
    /** Required: the dialog title. */
    title: string;
    onclose: () => void;
    children: Snippet;
  }

  let { open, title, onclose, children }: Props = $props();

  const titleId = `sheet-${Math.random().toString(36).slice(2, 9)}`;
  const DISMISS_PX = 96;
  let drag = $state(0);
  let startY = 0;
  let dragging = $state(false);

  function down(event: PointerEvent): void {
    dragging = true;
    startY = event.clientY;
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
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

{#if open}
  <button class="scrim" type="button" aria-label="Close" tabindex="-1" onclick={onclose}></button>
  <div
    class="sheet"
    role="dialog"
    aria-modal="true"
    aria-labelledby={titleId}
    style:--drag="{drag}px"
    class:dragging
    use:trapFocus={{ onEscape: onclose }}
  >
    <div
      class="grabber"
      role="presentation"
      onpointerdown={down}
      onpointermove={move}
      onpointerup={up}
      onpointercancel={up}
    ></div>
    <h2 id={titleId} class="title t-title">{title}</h2>
    <div class="body">
      {@render children()}
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 40;
    background: rgb(0 0 0 / 0.42);
    animation: fade-in var(--dur-base) var(--ease-out);
  }

  .sheet {
    position: fixed;
    z-index: 41;
    left: 0;
    right: 0;
    bottom: 0;
    display: flex;
    flex-direction: column;
    max-height: min(88dvh, var(--sheet-max));
    padding: var(--s-2) var(--gutter) calc(var(--safe-bottom) + var(--s-4));
    border-radius: var(--r-xl) var(--r-xl) 0 0;
    background: var(--surface);
    color: var(--on);
    box-shadow: var(--elev-2);
    transform: translateY(var(--drag));
    animation: sheet-in var(--spring-dur) var(--spring);
  }

  .sheet.dragging {
    transition: none;
  }

  .grabber {
    align-self: center;
    width: 48px;
    min-height: var(--hit);
    margin-bottom: var(--s-1);
    position: relative;
    touch-action: none;
  }

  .grabber::after {
    content: '';
    position: absolute;
    left: 50%;
    top: 50%;
    width: 36px;
    height: 5px;
    border-radius: var(--r-pill);
    background: var(--outline);
    transform: translate(-50%, -50%);
  }

  .title {
    padding: 0 var(--s-2) var(--s-3);
  }

  .body {
    overflow-y: auto;
    overscroll-behavior: contain;
    min-height: 0;
    user-select: text;
    -webkit-user-select: text;
  }

  @keyframes sheet-in {
    from {
      transform: translateY(100%);
    }
  }

  @keyframes fade-in {
    from {
      opacity: 0;
    }
  }

  /* Desktop and wide tablets: a centred panel instead of a bottom sheet. */
  @media (min-width: 900px) {
    .sheet {
      left: 50%;
      right: auto;
      top: 50%;
      bottom: auto;
      width: min(560px, calc(100vw - 2 * var(--s-8)));
      max-height: min(80dvh, var(--sheet-max));
      border-radius: var(--r-xl);
      padding-top: var(--s-4);
      transform: translate(-50%, -50%);
      animation: panel-in var(--spring-dur) var(--spring);
    }

    .grabber {
      display: none;
    }

    @keyframes panel-in {
      from {
        opacity: 0;
        transform: translate(-50%, -46%);
      }
    }
  }
</style>
