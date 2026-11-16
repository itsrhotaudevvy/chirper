<script lang="ts">
  /* Actions for one message: quick reactions, then reply, copy, edit (own), delete (own). */
  import type { Message } from '../../lib/core';
  import Icon from '../../lib/components/Icon.svelte';
  import Sheet from '../../lib/components/Sheet.svelte';
  import { QUICK_REACTIONS } from './emoji';

  export type MessageAction = 'reply' | 'copy' | 'edit' | 'delete' | 'more-reactions';

  interface Props {
    message: Message | null;
    onreact: (key: string) => void;
    onaction: (action: MessageAction) => void;
    onclose: () => void;
  }

  let { message, onreact, onaction, onclose }: Props = $props();

  const own = $derived(message?.fromMe ?? false);
  const canEdit = $derived(own && message?.type === 'text' && !message?.redacted);
  const canDelete = $derived(own && !message?.redacted);
</script>

<Sheet open={message !== null} title="Message" {onclose}>
  <div class="quick" role="group" aria-label="React">
    {#each QUICK_REACTIONS as emoji (emoji)}
      <button class="quick-cell" type="button" aria-label="React {emoji}" onclick={() => onreact(emoji)}>{emoji}</button>
    {/each}
    <button class="quick-cell more" type="button" aria-label="More reactions" onclick={() => onaction('more-reactions')}>
      <Icon name="plus" size={20} />
    </button>
  </div>

  <ul class="rows">
    <li><button class="row" type="button" onclick={() => onaction('reply')}><Icon name="reply" /> Reply</button></li>
    <li><button class="row" type="button" onclick={() => onaction('copy')}><Icon name="copy" /> Copy</button></li>
    {#if canEdit}
      <li><button class="row" type="button" onclick={() => onaction('edit')}><Icon name="edit" /> Edit</button></li>
    {/if}
    {#if canDelete}
      <li><button class="row danger" type="button" onclick={() => onaction('delete')}><Icon name="trash" /> Delete</button></li>
    {/if}
  </ul>
</Sheet>

<style>
  .quick {
    display: grid;
    grid-template-columns: repeat(7, minmax(0, 1fr));
    gap: var(--s-1);
    margin-bottom: var(--s-3);
  }

  .quick-cell {
    display: grid;
    place-items: center;
    min-height: var(--hit);
    border-radius: var(--r-pill);
    background: var(--surface-high);
    font-size: var(--t-title-size);
    transition: transform var(--dur-quick) var(--ease-out);
  }

  .quick-cell.more {
    color: var(--on-muted);
  }

  .quick-cell:active {
    transform: scale(0.9);
  }

  .rows {
    display: grid;
    gap: 2px;
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    width: 100%;
    min-height: var(--hit);
    padding: 0 var(--s-2);
    border-radius: var(--r-sm);
    font-weight: 600;
    text-align: start;
  }

  @media (hover: hover) and (pointer: fine) {
    .row:hover {
      background: var(--surface-high);
    }
  }

  .row.danger {
    color: var(--danger);
  }
</style>
