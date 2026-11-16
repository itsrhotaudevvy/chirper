<script lang="ts">
  interface Props {
    /** CSS colour of the band: a bright banner colour (bannerCss). */
    color: string;
    /** One bold word, for example a network name or "Pinned". */
    label: string;
  }

  let { color, label }: Props = $props();
</script>

<div class="band" role="presentation" style:--band={color}>
  <span class="word">{label}</span>
</div>

<style>
  /* DESIGN 3.3 v2 at list size: a slim solid band in the bright network colour, one soft highlight. */
  .band {
    position: relative;
    isolation: isolate;
    display: flex;
    align-items: center;
    height: var(--band-h);
    padding: 0 var(--gutter);
    background: var(--band);
    color: var(--on-banner);
    overflow: hidden;
  }

  .band::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    background: radial-gradient(70% 220% at 100% 0%, rgb(255 255 255 / 0.22), transparent 70%);
  }

  .word {
    font-size: var(--t-label-size);
    line-height: var(--t-label-line);
    font-weight: 750;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
</style>
