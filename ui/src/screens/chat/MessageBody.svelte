<script lang="ts">
  /*
   * What a message shows: plain text on the page (no bubble), a jumbo emoji, or a container (box)
   * for media, files, voice, and quoted replies. Redacted messages read "Deleted".
   */
  import type { Message } from '../../lib/core';
  import { personHue } from '../../lib/spectrum';
  import { formatBytes, mediaTypeOf } from './format';
  import AudioPlayer from './AudioPlayer.svelte';
  import MediaView from './MediaView.svelte';

  interface Props {
    message: Message;
    /** Jumbo font size for emoji-only text, or null. */
    jumbo: number | null;
    onopen: (src: string | null, kind: 'image' | 'video') => void;
  }

  let { message, jumbo, onopen }: Props = $props();

  const media = $derived(message.media);
  const kind = $derived(mediaTypeOf(message.type));
  const boxed = $derived(!message.redacted && (message.replyTo !== null || (media !== null && kind !== 'sticker')));
</script>

{#if message.redacted}
  <p class="deleted t-body">Deleted</p>
{:else if jumbo !== null}
  <p class="jumbo" style:font-size="{jumbo}px">{message.body}</p>
{:else if media && kind === 'sticker'}
  <MediaView {media} kind="sticker" {onopen} />
{:else if boxed}
  <div class="box">
    {#if message.replyTo}
      <div class="quote">
        <span class="bar" style:--bar="oklch(0.72 0.13 {personHue(message.replyTo.senderName)})" aria-hidden="true"></span>
        <span class="quote-text">
          <span class="quote-name t-label">{message.replyTo.senderName}</span>
          <span class="quote-body t-label">{message.replyTo.body}</span>
        </span>
      </div>
    {/if}

    {#if media}
      {#if kind === 'image' || kind === 'video'}
        <MediaView {media} {kind} {onopen} />
      {:else if kind === 'audio' || kind === 'voice'}
        <AudioPlayer {media} seed={message.eventId} />
      {:else}
        <div class="file">
          <span class="file-name t-headline truncate">{media.fileName || 'File'}</span>
          {#if media.size > 0}<span class="file-size t-label">{formatBytes(media.size)}</span>{/if}
        </div>
      {/if}
    {/if}

    {#if message.body && message.body !== media?.fileName}
      <p class="text t-body" class:caption={media !== null}>{message.body}</p>
    {/if}
  </div>
{:else if message.body}
  <p class="text t-body">{message.body}</p>
{:else}
  <p class="text t-body muted">[{message.type}]</p>
{/if}

<style>
  .deleted {
    color: var(--on-muted);
    font-style: italic;
  }

  .jumbo {
    line-height: 1.12;
    text-align: center;
  }

  .text {
    white-space: pre-wrap;
    word-break: normal;
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .text.caption {
    padding: var(--s-2) var(--s-2) var(--s-1);
  }

  .muted {
    color: var(--on-muted);
  }

  /* The container: radius and colour come from the row (--tl/--tr/--br/--bl) and the sender side. */
  .box {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    padding: 4px;
    border-radius: var(--tl, 18px) var(--tr, 18px) var(--br, 18px) var(--bl, 18px);
    background: var(--box-bg, var(--surface-high));
    overflow: hidden;
  }

  .box > :global(.tile) {
    border-radius: calc(var(--tl, 18px) - 4px) calc(var(--tr, 18px) - 4px) calc(var(--br, 18px) - 4px) calc(var(--bl, 18px) - 4px);
  }

  .box > .text {
    padding: var(--s-1) var(--s-2) var(--s-1);
  }

  .quote {
    display: flex;
    gap: var(--s-2);
    margin: var(--s-1) var(--s-2) var(--s-1);
    min-width: 0;
  }

  .bar {
    flex: none;
    width: 3px;
    border-radius: var(--r-pill);
    background: var(--bar);
  }

  .quote-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .quote-name {
    font-weight: 700;
    color: var(--bar);
  }

  .quote-body {
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
    color: var(--on-muted);
    word-break: normal;
    overflow-wrap: break-word;
    word-wrap: break-word;
  }

  .file {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 180px;
    padding: var(--s-2) var(--s-2) var(--s-1);
  }

  .file-name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-size {
    color: var(--on-muted);
  }
</style>
