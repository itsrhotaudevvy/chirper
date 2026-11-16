<script lang="ts">
  /*
   * The composer: a capsule over a translucent, blurred strip with a hairline on top, held to a
   * 760 px column. Attach and emoji appear only while the field is focused or empty. The send
   * button is always there, in the chat colour, and waits until there is text. Typing notifications
   * are sent at most every 3 s and stopped on send, blur, or 5 s idle.
   */
  import type { Message, MediaKind } from '../../lib/core';
  import { core } from '../../lib/core';
  import { haptic } from '../../lib/haptics';
  import Icon from '../../lib/components/Icon.svelte';
  import IconButton from '../../lib/components/IconButton.svelte';
  import EmojiSheet from './EmojiSheet.svelte';

  interface Props {
    roomId: string;
    /** Reply target, or null. */
    reply: Message | null;
    /** Message being edited, or null. Its text fills the field. */
    edit: Message | null;
    /** Newest own message: Up in an empty field edits it. */
    lastOwn: Message | null;
    /** Network colour of the chat. */
    ctx: string;
    onsubmit: (text: string) => void;
    oncancel: () => void;
    onedit: (m: Message) => void;
    onerror: (message: string) => void;
  }

  let { roomId, reply, edit, lastOwn, ctx, onsubmit, oncancel, onedit, onerror }: Props = $props();

  const TYPING_EVERY_MS = 3000;
  const TYPING_IDLE_MS = 5000;
  const MAX_LINES = 6;
  const LINE_PX = 21;

  let text = $state('');
  let focused = $state(false);
  /** Bumped on each send: the arrow turns once (DESIGN 5). */
  let turn = $state(0);
  let emojiOpen = $state(false);
  let field = $state<HTMLTextAreaElement | undefined>(undefined);
  let typingSent = false;
  let typingAt = 0;
  let idle: ReturnType<typeof setTimeout> | undefined;

  const hasText = $derived(text.trim().length > 0);
  const extras = $derived(focused || text.length === 0);
  const fine = typeof matchMedia === 'function' && matchMedia('(pointer: fine)').matches;
  const kindOfType = (mime: string): MediaKind => {
    if (mime.startsWith('image/')) return 'image';
    if (mime.startsWith('video/')) return 'video';
    if (mime.startsWith('audio/')) return 'audio';
    return 'file';
  };

  /* Edit fills the field; leaving edit clears it. */
  $effect(() => {
    if (edit) {
      text = edit.body;
      field?.focus();
    }
  });

  /* Auto-grow up to MAX_LINES. */
  $effect(() => {
    void text;
    if (!field) return;
    field.style.height = 'auto';
    field.style.height = `${Math.min(field.scrollHeight, MAX_LINES * LINE_PX + 20)}px`;
  });

  /** Moves the focus into the field (used when a reply starts). */
  export function focus(): void {
    field?.focus();
  }

  function stopTyping(): void {
    clearTimeout(idle);
    idle = undefined;
    if (!typingSent) return;
    typingSent = false;
    void core.setTyping({ roomId, typing: false }).catch(() => undefined);
  }

  function noteTyping(): void {
    if (!hasText) {
      stopTyping();
      return;
    }
    const now = Date.now();
    if (!typingSent || now - typingAt > TYPING_EVERY_MS) {
      typingSent = true;
      typingAt = now;
      void core.setTyping({ roomId, typing: true }).catch(() => undefined);
    }
    clearTimeout(idle);
    idle = setTimeout(stopTyping, TYPING_IDLE_MS);
  }

  $effect(() => () => stopTyping());

  function submit(): void {
    const body = text.trim();
    if (!body) return;
    stopTyping();
    haptic('light');
    text = '';
    turn++;
    onsubmit(body);
  }

  /** Cancels a reply or an edit; cancelling an edit also clears the text it put in the field. */
  function cancel(): void {
    if (edit) text = '';
    oncancel();
  }

  function onkeydown(event: KeyboardEvent): void {
    if (event.isComposing) return;
    if (event.key === 'Enter' && !event.shiftKey && fine) {
      event.preventDefault();
      submit();
    } else if (event.key === 'ArrowUp' && text === '' && !reply && lastOwn) {
      event.preventDefault();
      onedit(lastOwn);
    } else if (event.key === 'Escape' && (reply || edit)) {
      event.preventDefault();
      cancel();
    }
  }

  function insertEmoji(emoji: string): void {
    text += emoji;
    emojiOpen = false;
    field?.focus();
  }

  async function attach(): Promise<void> {
    try {
      const files = await core.pickFiles({ kind: 'media' });
      for (const file of files) {
        await core.sendMedia({
          roomId,
          path: file.path,
          mime: file.mime,
          fileName: file.name,
          caption: '',
          kind: kindOfType(file.mime),
          replyTo: reply?.eventId ?? '',
          txnId: crypto.randomUUID(),
        });
      }
    } catch (err: unknown) {
      onerror(err instanceof Error ? err.message : 'Could not attach the file');
    }
  }
</script>

<div class="composer">
  <div class="column">
  {#if reply || edit}
    {@const target = edit ?? reply}
    <div class="context" style:--strip={ctx}>
      <span class="context-text">
        <span class="context-title t-label">{edit ? 'Editing' : `Replying to ${reply?.fromMe ? 'yourself' : reply?.senderName ?? ''}`}</span>
        <span class="context-body t-label truncate">{target?.body || (target?.media ? target.type : '')}</span>
      </span>
      <IconButton icon="close" label={edit ? 'Cancel edit' : 'Cancel reply'} onclick={cancel} size={20} />
    </div>
  {/if}

  <div class="controls-bar">
    {#if extras}
      <span class="extra">
        <IconButton icon="attach" label="Attach" onclick={attach} size={24} />
      </span>
    {/if}

    <textarea
      bind:this={field}
      bind:value={text}
      rows="1"
      aria-label="Message"
      placeholder="Message"
      enterkeyhint="send"
      onfocus={() => (focused = true)}
      onblur={() => {
        focused = false;
        stopTyping();
      }}
      oninput={noteTyping}
      {onkeydown}
    ></textarea>

    {#if extras}
      <span class="extra">
        <IconButton icon="emoji" label="Emoji" onclick={() => (emojiOpen = true)} size={24} />
      </span>
    {/if}

    <button class="send" type="button" style:--ctx={ctx} aria-label="Send" disabled={!hasText} onclick={submit}>
      {#key turn}
        <span class="arrow" class:turned={turn > 0}><Icon name="send" size={24} /></span>
      {/key}
    </button>
  </div>
  </div>
</div>

<EmojiSheet open={emojiOpen} title="Emoji" onpick={insertEmoji} onclose={() => (emojiOpen = false)} />

<style>
  .composer {
    position: relative;
    flex: none;
    padding: var(--s-2) calc(var(--gutter) + var(--safe-right)) calc(var(--s-2) + var(--safe-bottom))
      calc(var(--gutter) + var(--safe-left));
    background: color-mix(in srgb, var(--surface) 92%, transparent);
    -webkit-backdrop-filter: blur(16px);
    backdrop-filter: blur(16px);
    border-top: 1px solid var(--outline);
  }

  /* The 760 px reading column, shared with the timeline. */
  .column {
    max-width: 760px;
    margin: 0 auto;
  }

  /* A reply or edit is a solid strip in the chat colour, with banner ink (DESIGN 6.4). */
  .context {
    display: flex;
    align-items: center;
    gap: var(--s-2);
    margin-bottom: var(--s-2);
    padding: var(--s-1) var(--s-1) var(--s-1) var(--s-3);
    border-radius: var(--r-md);
    background: var(--strip);
    color: var(--on-banner);
  }

  .context-text {
    display: flex;
    flex-direction: column;
    flex: 1 1 auto;
    min-width: 0;
  }

  .context-title {
    font-weight: 700;
  }

  .context-body {
    opacity: 0.85;
  }

  .context :global(.icon-button) {
    color: var(--on-banner);
  }

  .controls-bar {
    display: flex;
    align-items: flex-end;
    gap: var(--s-1);
    min-height: calc(var(--hit) + 4px);
    padding: 3px var(--s-1) 3px var(--s-2);
    border-radius: var(--r-xl);
    background: var(--surface-high);
    border: 1px solid var(--outline);
    transition: border-color var(--dur-quick) var(--ease-out), box-shadow var(--dur-quick) var(--ease-out);
  }

  .controls-bar:focus-within {
    border-color: color-mix(in srgb, var(--ctx, var(--on)) 40%, var(--outline));
    box-shadow: 0 0 0 2px color-mix(in srgb, var(--ctx, var(--on)) 12%, transparent);
  }

  .extra {
    display: inline-flex;
    flex: none;
    animation: appear var(--dur-base) var(--ease-out) both;
  }

  @keyframes appear {
    from {
      opacity: 0;
      transform: scale(0.8);
    }
  }

  textarea {
    flex: 1 1 auto;
    min-width: 0;
    min-height: var(--hit);
    max-height: calc(6 * 21px + 20px);
    padding: 11px var(--s-2);
    resize: none;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--on);
    font-size: var(--t-body-size);
    line-height: var(--t-body-line);
    overflow-y: auto;
  }

  textarea::placeholder {
    color: var(--on-muted);
  }

  .send {
    position: relative;
    flex: none;
    display: grid;
    place-items: center;
    width: var(--send-size);
    height: var(--send-size);
    border-radius: 50%;
    background: var(--ctx);
    color: var(--on-banner);
    transition:
      transform var(--dur-quick) var(--ease-out),
      opacity var(--dur-base) var(--ease-out);
  }

  .send:active:not(:disabled) {
    transform: scale(0.92);
  }

  .send:disabled {
    opacity: 0.4;
  }

  /* On send the arrow turns a quarter (DESIGN 5), from -90 deg to 0. */
  .arrow {
    display: grid;
    place-items: center;
  }

  .arrow.turned {
    animation: send-turn var(--dur-base) var(--ease-out) both;
  }

  @keyframes send-turn {
    from {
      transform: rotate(-90deg);
    }
  }
</style>
