<script lang="ts">
  import { settings, type ThemePref } from '../../lib/stores';
  import { ACCENT_PRESETS } from '../../lib/spectrum';
  import Segmented from '../../lib/components/Segmented.svelte';
  import Toggle from '../../lib/components/Toggle.svelte';
  import { swatchGradient } from './accents';

  const COLOUR = 'var(--cat-appearance)';
  const THEMES: ReadonlyArray<{ value: ThemePref; label: string }> = [
    { value: 'system', label: 'System' },
    { value: 'dark', label: 'Dark' },
    { value: 'light', label: 'Light' },
    { value: 'amoled', label: 'AMOLED' },
  ];
</script>

<div class="block">
  <span class="label t-label">Theme</span>
  <Segmented
    label="Theme"
    options={THEMES}
    value={settings.theme}
    onchange={(theme) => settings.updateLocal({ theme })}
  />
</div>

<div class="block">
  <span class="label t-label">Accent</span>
  <div class="swatches" role="radiogroup" aria-label="Accent">
    {#each ACCENT_PRESETS as preset (preset.id)}
      <button
        class="swatch"
        class:selected={settings.accent === preset.id}
        type="button"
        role="radio"
        aria-checked={settings.accent === preset.id}
        aria-label={preset.label}
        style:--swatch={swatchGradient(preset.id)}
        onclick={() => settings.updateLocal({ accent: preset.id })}
      ></button>
    {/each}
  </div>
</div>

<div class="preview" aria-hidden="true">
  <div class="mini-banner">
    <span class="mini-title t-headline">chirper</span>
  </div>
  <div class="lines">
    <p class="line in t-body">Dinner at eight?</p>
    <p class="line out t-body">Yes, see you there.</p>
    <p class="line in t-body">Bring the photos.</p>
  </div>
</div>

<Toggle label="Reduce motion" color={COLOUR} checked={settings.reduceMotion} onchange={(on) => settings.updateLocal({ reduceMotion: on })} />

<style>
  .block {
    display: grid;
    gap: var(--s-2);
    padding: 0 var(--gutter);
  }

  .label {
    color: var(--on-muted);
  }

  .swatches {
    display: flex;
    gap: var(--s-3);
    overflow-x: auto;
    scroll-snap-type: x proximity;
    padding: var(--s-1) 2px var(--s-2);
    scrollbar-width: none;
  }

  .swatches::-webkit-scrollbar {
    display: none;
  }

  .swatch {
    flex: none;
    width: var(--hit);
    height: var(--hit);
    border-radius: 50%;
    background: var(--swatch);
    scroll-snap-align: center;
    box-shadow: inset 0 0 0 2px rgb(255 255 255 / 0.12);
    transition: transform var(--dur-quick) var(--ease-out);
  }

  .swatch.selected {
    outline: 3px solid var(--on);
    outline-offset: 2px;
  }

  .swatch:active {
    transform: scale(0.94);
  }

  /* Live preview: reads the same tokens the app uses, so it changes the moment a choice is made. */
  .preview {
    margin: 0 var(--gutter);
    border-radius: var(--r-lg);
    overflow: hidden;
    background: var(--bg);
    box-shadow: inset 0 0 0 1px var(--outline);
  }

  /* The accent preview: the accent gradient as a banner, with the accent's own ink. */
  .mini-banner {
    display: flex;
    align-items: flex-end;
    min-height: 72px;
    padding: var(--s-3) var(--gutter);
    background:
      radial-gradient(120% 160% at 100% 0%, rgb(255 255 255 / 0.22) 0%, transparent 70%),
      linear-gradient(100deg, var(--accent-2), var(--accent-3), var(--accent-4));
    color: var(--on-accent);
  }

  .mini-title {
    font-weight: 700;
  }

  .lines {
    display: grid;
    gap: var(--s-2);
    padding: var(--s-3);
  }

  .line {
    max-width: 75%;
    padding: var(--s-1) 0;
    color: var(--on);
  }

  .line.out {
    justify-self: end;
    color: color-mix(in srgb, var(--accent-2) 35%, var(--on));
    text-align: end;
  }
</style>
