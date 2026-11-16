<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    /** Id of the wordmark heading, for the screen's aria-labelledby. */
    id: string;
    /** Shorter hero, for screens with a control below it. */
    short?: boolean;
    /** Text under the wordmark, on the colour (one sentence at most). */
    children?: Snippet;
  }

  let { id, short = false, children }: Props = $props();

  /** The 14 bright spectrum colours in order: one rainbow across the hero. */
  const STOPS = Array.from({ length: 14 }, (_, i) => `var(--banner-spectrum-${i})`).join(', ');
</script>

<section class="hero" class:short>
  <!-- Two copies of the stops, twice the width: the drift loops seamlessly. -->
  <div class="rainbow" aria-hidden="true" style:--stops={STOPS}></div>
  <div class="content">
    <svg class="mark" viewBox="0 0 108 108" aria-hidden="true">
      <path d="M40,28 H68 A14,14 0 0 1 82,42 V56 A14,14 0 0 1 68,70 H59 L46,80 V70 H40 A14,14 0 0 1 26,56 V42 A14,14 0 0 1 40,28 Z" />
    </svg>
    <h1 {id} class="wordmark">chirper</h1>
    {@render children?.()}
  </div>
</section>

<style>
  /* DESIGN 3.3 v2: a solid bright banner with a crisp bottom edge and one soft highlight. */
  .hero {
    position: relative;
    isolation: isolate;
    flex: none;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    height: min(62dvh, 560px);
    min-height: 300px;
    border-radius: 0 0 var(--banner-radius) var(--banner-radius);
    overflow: hidden;
    color: var(--on-banner);
  }

  .hero.short {
    height: min(46dvh, 420px);
    min-height: 240px;
  }

  .rainbow {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    width: 200%;
    z-index: -2;
    background: linear-gradient(90deg, var(--stops), var(--stops));
    animation: drift 48s linear infinite;
  }

  .hero::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    pointer-events: none;
    background: radial-gradient(90% 60% at 15% 0%, rgb(255 255 255 / 0.22), transparent 70%);
  }

  @keyframes drift {
    to {
      transform: translateX(-50%);
    }
  }

  .content {
    display: grid;
    justify-items: start;
    gap: var(--s-3);
    padding: calc(var(--safe-top) + var(--s-12)) calc(var(--gutter) + var(--safe-right)) var(--s-6)
      calc(var(--gutter) + var(--safe-left));
  }

  .mark {
    width: 56px;
    height: 56px;
    fill: currentColor;
  }

  .wordmark {
    margin: 0;
    font-size: var(--t-hero-size);
    line-height: var(--t-hero-line);
    font-weight: 700;
    letter-spacing: -0.02em;
  }

  @media (prefers-reduced-motion: reduce) {
    .rainbow {
      animation: none;
    }
  }

  :root[data-reduce-motion='true'] .rainbow {
    animation: none;
  }
</style>
