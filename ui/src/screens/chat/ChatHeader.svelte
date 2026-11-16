<script lang="ts">
  /*
   * The chat's banner (DESIGN 4, Banner) and its compact bar. At rest the banner is the full hero
   * with the chat name. As the reader scrolls away from the newest message (progress 0 -> 1) the
   * banner slides up under a solid bar of the same colour: transform and opacity only.
   */
  import type { Chat } from '../../lib/core';
  import Avatar from '../../lib/components/Avatar.svelte';
  import Banner from '../../lib/components/Banner.svelte';
  import IconButton from '../../lib/components/IconButton.svelte';
  import TypingDots from '../../lib/components/TypingDots.svelte';

  interface Props {
    /** Undefined while the chat is unknown (for example just removed). */
    chat: Chat | undefined;
    /** Network colour (CSS colour). */
    ctx: string;
    /** 0 = full banner, 1 = compact bar. */
    progress: number;
    /** Phone and tablet only: the split view has its own inbox to return to. */
    showBack: boolean;
    onback: () => void;
    onmore: () => void;
  }

  let { chat, ctx, progress, showBack, onback, onmore }: Props = $props();

  const name = $derived(chat?.name || 'Chat');
  const typing = $derived((chat?.typing.length ?? 0) > 0);
  const typingLabel = $derived(chat && chat.typing.length > 0 ? `${chat.typing.join(', ')} typing` : 'Typing');
</script>

<header class="head" style:--ctx={ctx} style:--p={progress}>
  <div class="hero" aria-hidden={progress > 0.5 ? 'true' : undefined}>
    <Banner colors={[ctx]} title={name}>
      {#snippet children()}
        {#if typing}
          <span class="typing t-label">typing <TypingDots label={typingLabel} /></span>
        {/if}
      {/snippet}
    </Banner>
  </div>

  <div class="bar">
    {#if showBack}
      <IconButton icon="back" label="Back" onclick={onback} />
    {/if}
    <span class="compact-avatar">
      <Avatar name={name} id={chat?.roomId} src={chat?.avatarUrl} size={36} />
    </span>
    <h2 class="compact-name t-headline truncate">{name}</h2>
    <IconButton icon="more" label="Chat options" onclick={onmore} />
  </div>
</header>

<style>
  .head {
    position: absolute;
    z-index: 2;
    top: 0;
    left: 0;
    right: 0;
    height: var(--head-big);
    pointer-events: none;
    --compact-h: calc(48px + var(--safe-top) + var(--titlebar-inset));
  }

  .hero {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    transform: translateY(calc(var(--p) * (var(--compact-h) - var(--head-big))));
    opacity: calc(1 - var(--p));
    will-change: transform, opacity;
  }

  .hero :global(.banner) {
    pointer-events: none;
  }

  /* Typing sits on the banner as plain text, like the title: no box behind it. */
  .typing {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    justify-self: start;
    color: var(--on-banner);
  }

  .bar {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: var(--compact-h);
    display: flex;
    align-items: center;
    gap: var(--s-2);
    padding: var(--titlebar-inset) calc(var(--gutter) + var(--safe-right)) 0 calc(var(--gutter) + var(--safe-left));
    color: var(--on-banner);
    pointer-events: auto;
    isolation: isolate;
  }

  /* The solid banner colour of the compact bar fades in over the banner. */
  .bar::before {
    content: '';
    position: absolute;
    inset: 0;
    z-index: -1;
    background: var(--ctx);
    opacity: var(--p);
  }

  .bar :global(.icon-button) {
    color: var(--on-banner);
  }

  /* At rest the banner shows the name; the compact bar gains its avatar as it takes over. */
  .compact-avatar {
    display: inline-grid;
    flex: none;
    opacity: var(--p);
  }

  .compact-name {
    flex: 1 1 auto;
    min-width: 0;
    margin-left: var(--s-1);
    opacity: var(--p);
    color: var(--on-banner);
  }

  @media (min-width: 900px) {
    .head {
      --compact-h: calc(48px + var(--safe-top) + var(--titlebar-inset));
    }
  }
</style>
