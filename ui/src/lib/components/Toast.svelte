<script lang="ts">
  import { announce } from '../a11y';
  import StatusStrip from './StatusStrip.svelte';

  interface Props {
    message: string;
    tone?: 'info' | 'warn' | 'danger';
    action?: { label: string; onclick: () => void };
    /** Milliseconds before the toast asks to be removed. Errors stay until dismissed. */
    duration?: number;
    ondismiss: () => void;
  }

  let { message, tone = 'info', action, duration = 4000, ondismiss }: Props = $props();

  $effect(() => {
    announce(message, tone === 'danger' ? 'assertive' : 'polite');
    if (tone === 'danger') return undefined;
    const timer = setTimeout(ondismiss, duration);
    return () => clearTimeout(timer);
  });
</script>

<div class="toast">
  <StatusStrip {tone} {message} action={action} />
</div>

<style>
  .toast {
    position: fixed;
    left: var(--gutter);
    right: var(--gutter);
    bottom: calc(var(--safe-bottom) + var(--s-4));
    z-index: 60;
    max-width: 480px;
    margin-inline: auto;
    box-shadow: var(--elev-1);
    animation: rise var(--spring-dur) var(--spring);
  }

  @keyframes rise {
    from {
      transform: translateY(16px);
      opacity: 0;
    }
  }
</style>
