<script lang="ts">
  /* Compact voice or audio player: a play button and a bar of 28 strokes in the chat colour. */
  import type { MessageMedia } from '../../lib/core';
  import { mediaUrl } from '../../lib/media';
  import { formatDuration } from './format';

  interface Props {
    media: MessageMedia;
    /** Seeds the waveform so the same message always draws the same bars. */
    seed: string;
  }

  let { media, seed }: Props = $props();

  const BARS = 28;
  const src = $derived(mediaUrl(media.url));
  let audio = $state<HTMLAudioElement | undefined>(undefined);
  let playing = $state(false);
  let progress = $state(0);
  let elapsed = $state(0);

  /* Deterministic pseudo-random bar heights in 30..100 %, from the seed (FNV-1a per bar). */
  const heights = $derived(
    Array.from({ length: BARS }, (_, i) => {
      let h = 0x811c9dc5 ^ i;
      for (let c = 0; c < seed.length; c++) h = Math.imul(h ^ seed.charCodeAt(c), 0x01000193);
      return 30 + ((h >>> 0) % 71);
    }),
  );

  function toggle(): void {
    if (!audio) return;
    if (audio.paused) void audio.play().catch(() => (playing = false));
    else audio.pause();
  }

  function onTime(): void {
    if (!audio) return;
    elapsed = audio.currentTime * 1000;
    const total = audio.duration && Number.isFinite(audio.duration) ? audio.duration : media.durationMs / 1000;
    progress = total > 0 ? Math.min(1, audio.currentTime / total) : 0;
  }
</script>

<div class="player">
  <button class="play" type="button" aria-label={playing ? 'Pause' : 'Play'} disabled={!src} onclick={toggle}>
    <svg viewBox="0 0 24 24" aria-hidden="true">
      {#if playing}
        <path d="M7 5.5h3.5v13H7zM13.5 5.5H17v13h-3.5z" />
      {:else}
        <path d="M8 5.5v13l10.5-6.5z" />
      {/if}
    </svg>
  </button>
  <span class="wave" aria-hidden="true">
    {#each heights as h, i (i)}
      <span class="stroke" class:played={i / BARS < progress} style:height="{h}%"></span>
    {/each}
  </span>
  <span class="time t-label">{formatDuration(playing || elapsed > 0 ? elapsed : media.durationMs)}</span>
  {#if src}
    <audio
      bind:this={audio}
      src={src}
      preload="metadata"
      ontimeupdate={onTime}
      onplay={() => (playing = true)}
      onpause={() => (playing = false)}
      onended={() => {
        playing = false;
        progress = 0;
        elapsed = 0;
      }}
    ></audio>
  {/if}
</div>

<style>
  .player {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    min-width: 200px;
    padding: var(--s-1) var(--s-1) var(--s-1) 0;
  }

  .play {
    display: grid;
    place-items: center;
    flex: none;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--ctx);
    transition: transform var(--dur-quick) var(--ease-out);
  }

  .play:active:not(:disabled) {
    transform: scale(0.92);
  }

  .play:disabled {
    opacity: 0.4;
  }

  .play svg {
    width: 18px;
    height: 18px;
    fill: var(--on-banner);
  }

  .wave {
    display: flex;
    align-items: center;
    gap: 2px;
    height: 28px;
    flex: 1 1 auto;
    min-width: 0;
  }

  .stroke {
    flex: 1 1 0;
    min-width: 2px;
    max-width: 4px;
    border-radius: var(--r-pill);
    background: color-mix(in srgb, var(--on) 28%, transparent);
  }

  .stroke.played {
    background: var(--ctx);
  }

  .time {
    flex: none;
    color: var(--on-muted);
    font-variant-numeric: tabular-nums;
  }
</style>
