<script lang="ts">
  /* Reaction pills under a message: 24 px, own reactions filled with the chat colour. */
  import type { Message } from '../../lib/core';
  import { haptic } from '../../lib/haptics';

  interface Props {
    message: Message;
    /** Toggles the reaction (react when not yet ours, unreact when it is). */
    ontoggle: (key: string) => void;
  }

  let { message, ontoggle }: Props = $props();
  /** The pill that was just tapped plays the pop animation (transform only). */
  let popped = $state('');

  function tap(key: string): void {
    haptic('light');
    popped = key;
    ontoggle(key);
  }
</script>

<div class="reactions" role="group" aria-label="Reactions">
  {#each message.reactions as r (r.key)}
    <button
      class="pill t-label"
      class:mine={r.fromMe}
      class:pop={popped === r.key}
      type="button"
      aria-pressed={r.fromMe}
      aria-label="{r.key} {r.count}{r.fromMe ? ', you reacted' : ''}"
      onclick={() => tap(r.key)}
      onanimationend={() => (popped = '')}
    >
      <span class="emoji">{r.key}</span>
      {#if r.count > 1}<span class="count">{r.count}</span>{/if}
    </button>
  {/each}
</div>

<style>
  .reactions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--s-1);
    max-width: 100%;
  }

  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 var(--s-2);
    border-radius: var(--r-pill);
    background: var(--surface-high);
    color: var(--on);
    box-shadow: 0 0 0 1px var(--outline) inset;
    font-weight: 600;
  }

  .pill.mine {
    background: var(--ctx);
    color: var(--on-banner);
    box-shadow: none;
  }

  .emoji {
    font-size: var(--t-body-size);
    line-height: 1;
  }

  .pill.pop {
    animation: pop var(--spring-bounce-dur) var(--spring-bounce) both;
  }

  @keyframes pop {
    0% {
      transform: scale(0.7);
    }
    35% {
      transform: scale(1.1);
    }
    100% {
      transform: scale(1);
    }
  }
</style>
