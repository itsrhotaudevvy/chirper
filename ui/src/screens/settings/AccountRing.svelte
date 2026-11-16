<script lang="ts">
  import Avatar from '../../lib/components/Avatar.svelte';
  import type { Account } from '../../lib/core';
  import { accountRing } from '../../lib/stores/accounts.svelte';

  interface Props {
    account: Account;
    size?: 36 | 48;
  }

  let { account, size = 36 }: Props = $props();

  const ring = $derived(accountRing(account));
</script>

<span class="ring {ring.tone}" style:--ring-c={ring.colour}>
  <Avatar name={account.name || account.network} id={account.loginId} src={account.avatarUrl} {size} />
</span>

<style>
  /* State by colour: solid = connected, pulsing = connecting, warn/danger = needs attention. */
  .ring {
    position: relative;
    display: inline-grid;
    place-items: center;
    flex: none;
    padding: 4px;
  }

  .ring::before {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: 50%;
    border: 3px solid var(--ring-c);
  }

  .ring.pulse::before {
    animation: breathe 1.6s ease-in-out infinite;
  }

  @keyframes breathe {
    50% {
      opacity: 0.5;
    }
  }
</style>
