<script lang="ts">
  /* Split view with nothing selected: one centred line over a faint rainbow wash (DESIGN 3.2). */
  const wash = Array.from(
    { length: 14 },
    (_, i) => `color-mix(in srgb, var(--banner-spectrum-${i}) 16%, transparent) ${((i / 13) * 100).toFixed(1)}%`,
  ).join(', ');
</script>

<section class="empty" aria-label="No chat selected">
  <div class="wash" style:background-image="linear-gradient(115deg, {wash})" aria-hidden="true"></div>
  <p class="line t-headline">Pick a chat</p>
</section>

<style>
  .empty {
    position: relative;
    display: grid;
    place-items: center;
    height: 100%;
    min-height: 0;
    background: var(--bg);
    overflow: hidden;
  }

  .wash {
    position: absolute;
    inset: 0;
    opacity: 0.45;
    filter: blur(48px);
    transform: scale(1.2);
  }

  .line {
    position: relative;
    margin: 0;
    color: var(--on-muted);
  }
</style>
