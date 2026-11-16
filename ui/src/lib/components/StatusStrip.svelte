<script lang="ts">
  interface Props {
    /** warn and danger are banner states (colour + one word); info is neutral. */
    tone?: 'info' | 'warn' | 'danger';
    message: string;
    /** Optional single action word. */
    action?: { label: string; onclick: () => void };
  }

  let { tone = 'info', message, action }: Props = $props();
</script>

<div class="strip {tone}" role={tone === 'danger' ? 'alert' : 'status'}>
  <span class="message t-label">{message}</span>
  {#if action}
    <button class="action t-label" type="button" onclick={action.onclick}>{action.label}</button>
  {/if}
</div>

<style>
  .strip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--s-3);
    min-height: var(--hit);
    padding: var(--s-2) var(--s-4);
    border-radius: var(--r-md);
    background: var(--surface-high);
    color: var(--on);
  }

  /* Banner states: solid bright colour and banner ink, like the banners they stand in for. */
  .strip.warn {
    background: var(--banner-warn);
    color: var(--on-banner);
  }

  .strip.danger {
    background: var(--banner-danger);
    color: var(--on-banner);
  }

  .message {
    min-width: 0;
    font-weight: 600;
  }

  .action {
    flex: none;
    min-height: var(--hit);
    padding: 0 var(--s-3);
    border-radius: var(--r-pill);
    color: inherit;
    font-weight: 700;
    text-decoration: underline;
    text-underline-offset: 3px;
  }
</style>
