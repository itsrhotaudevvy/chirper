<script lang="ts">
  /*
   * App shell. Phone and tablet: one view at a time, moves slide (DESIGN 5). Split (>= 900 px):
   * resizable inbox pane (300-480 px) and a detail pane. Lock screen and onboarding overlay both.
   */
  import { onMount } from 'svelte';
  import { connectStores, accounts, chats, ui, settings } from './lib/stores';
  import { createBadgeSync } from './lib/stores/chats.svelte';
  import { router, parseDeepLink } from './lib/router.svelte';
  import { core } from './lib/core';
  import { INBOX_WIDTH } from './lib/stores/ui.svelte';
  import { updater } from './lib/updater';
  import InboxScreen from './screens/inbox/InboxScreen.svelte';
  import ChatScreen from './screens/chat/ChatScreen.svelte';
  import EmptyChat from './screens/chat/EmptyChat.svelte';
  import SettingsScreen from './screens/settings/SettingsScreen.svelte';
  import AddNetwork from './screens/login/AddNetwork.svelte';
  import Onboarding from './screens/onboarding/Onboarding.svelte';
  import LockScreen from './screens/lock/LockScreen.svelte';

  let ready = $state(false);
  let locked = $state(false);
  let onboardingDone = $state(false);

  const route = $derived(router.route);
  const split = $derived(ui.mode === 'split');
  const needsOnboarding = $derived(ready && !onboardingDone && accounts.list.length === 0);
  /** Changes on every move, so the phone pane is re-created and its enter animation runs. */
  const screenKey = $derived(route.view === 'chat' ? `chat:${route.roomId ?? ''}` : route.view);

  const badge = createBadgeSync((count: number) => core.setBadge({ count }));
  $effect(() => {
    badge.update(chats.unreadTotal);
  });

  /** Platform and form factor become attributes on <html>, which the CSS reads (titlebar inset). */
  function markPlatform(): void {
    const root = document.documentElement;
    root.classList.toggle('vt', 'startViewTransition' in document);
    void core
      .appInfo()
      .then((info) => {
        root.dataset.platform = info.platform;
        root.dataset.form = info.formFactor;
      })
      .catch(() => undefined);
  }

  /** Inbox pane resize: pointer drag, or the arrow keys on the separator. Persisted on release. */
  function startResize(event: PointerEvent): void {
    const handle = event.currentTarget as HTMLElement;
    handle.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = ui.inboxWidth;
    const onMove = (e: PointerEvent): void => ui.setInboxWidth(startWidth + e.clientX - startX);
    const onUp = (): void => {
      ui.setInboxWidth(ui.inboxWidth, true);
      handle.removeEventListener('pointermove', onMove);
      handle.removeEventListener('pointerup', onUp);
      handle.removeEventListener('pointercancel', onUp);
    };
    handle.addEventListener('pointermove', onMove);
    handle.addEventListener('pointerup', onUp);
    handle.addEventListener('pointercancel', onUp);
  }

  function nudgeResize(event: KeyboardEvent): void {
    const step = event.key === 'ArrowLeft' ? -16 : event.key === 'ArrowRight' ? 16 : 0;
    if (step === 0) return;
    event.preventDefault();
    ui.setInboxWidth(ui.inboxWidth + step, true);
  }

  onMount(() => {
    markPlatform();
    const disposers: Array<() => void> = [ui.attach(), router.attach(), badge.dispose];
    disposers.push(core.onAppLock((s) => (locked = s.locked)));
    void core.lockStatus().then((s) => (locked = s.locked)).catch(() => undefined);
    disposers.push(
      core.onDeepLink((url) => {
        const target = parseDeepLink(url);
        if (target?.roomId) router.openChat(target.roomId);
      }),
    );
    disposers.push(
      core.onNotificationReply(({ roomId, text }) => {
        void core.sendText({ roomId, text, replyTo: '', txnId: crypto.randomUUID() }).catch(() => undefined);
      }),
    );
    const initStores = () => {
      void connectStores()
        .then((dispose) => {
          disposers.push(dispose);
          ready = true;
          if (settings.autoUpdate) {
            void updater.checkForUpdates({ autoDownload: true, silent: true });
          }
        })
        .catch((err) => {
          console.warn('Initial connectStores error, retrying in 500ms:', err);
          setTimeout(initStores, 500);
        });
    };
    initStores();
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && router.view !== 'inbox') router.back();
    };
    window.addEventListener('keydown', onKey);
    disposers.push(() => window.removeEventListener('keydown', onKey));
    return () => disposers.forEach((d) => d());
  });
</script>

{#snippet detail()}
  {#if route.view === 'chat' && route.roomId}
    {#key route.roomId}
      <ChatScreen roomId={route.roomId} onback={() => router.back()} />
    {/key}
  {:else if route.view === 'settings'}
    <SettingsScreen onclose={() => router.back()} />
  {:else if route.view === 'add-network'}
    <AddNetwork network={route.network} onclose={() => router.back()} />
  {:else}
    <EmptyChat />
  {/if}
{/snippet}

<div class="app" class:split class:tablet={ui.mode === 'tablet'}>
  {#if split}
    <aside class="pane inbox" style:width="{ui.inboxWidth}px"><InboxScreen /></aside>
    <div
      class="handle"
      role="slider"
      aria-orientation="horizontal"
      aria-label="Resize chat list"
      aria-valuemin={INBOX_WIDTH.min}
      aria-valuemax={INBOX_WIDTH.max}
      aria-valuenow={ui.inboxWidth}
      tabindex="0"
      onpointerdown={startResize}
      onkeydown={nudgeResize}
    ></div>
    <main class="pane detail">{@render detail()}</main>
  {:else}
    {#key screenKey}
      <main class="pane enter">
        {#if route.view === 'inbox'}<InboxScreen />{:else}{@render detail()}{/if}
      </main>
    {/key}
  {/if}

  {#if needsOnboarding && route.view !== 'add-network'}
    <div class="overlay"><Onboarding ondone={() => (onboardingDone = true)} /></div>
  {/if}
  {#if locked}
    <div class="overlay"><LockScreen /></div>
  {/if}
</div>

<style>
  .app {
    position: fixed;
    inset: 0;
    display: flex;
    background: var(--bg);
    color: var(--on);
    overflow: hidden;
  }

  .pane {
    position: relative;
    flex: 1 1 auto;
    min-width: 0;
    height: 100%;
    overflow: hidden;
  }

  .split .inbox {
    flex: 0 0 auto;
  }

  /* Split view: a hairline with a wider invisible grab area. */
  .handle {
    position: relative;
    flex: 0 0 9px;
    margin-left: -4px;
    margin-right: -4px;
    z-index: 2;
    cursor: col-resize;
    touch-action: none;
  }

  .handle::after {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 4px;
    width: 1px;
    background: var(--outline);
  }

  .handle:focus-visible {
    outline-offset: -2px;
  }

  .overlay {
    position: absolute;
    inset: 0;
    z-index: 50;
    background: var(--bg);
  }

  /* Without the View Transitions API, a moved-to pane slides in (280 ms). */
  :global(html:not(.vt)) .pane.enter {
    animation: chirper-in-right var(--dur-page) var(--ease-out) both;
  }

  :global(html[data-nav='back']:not(.vt)) .pane.enter {
    animation-name: chirper-in-left;
  }

  :global(html[data-nav='forward'])::view-transition-old(root) {
    animation: chirper-out-left var(--dur-page) var(--ease-out) both;
  }

  :global(html[data-nav='forward'])::view-transition-new(root) {
    animation: chirper-in-right var(--dur-page) var(--ease-out) both;
  }

  :global(html[data-nav='back'])::view-transition-old(root) {
    animation: chirper-out-right var(--dur-page) var(--ease-out) both;
  }

  :global(html[data-nav='back'])::view-transition-new(root) {
    animation: chirper-in-left var(--dur-page) var(--ease-out) both;
  }

  @keyframes -global-chirper-in-right {
    from {
      transform: translateX(28%);
      opacity: 0.4;
    }
    to {
      transform: none;
      opacity: 1;
    }
  }

  @keyframes -global-chirper-in-left {
    from {
      transform: translateX(-28%);
      opacity: 0.4;
    }
    to {
      transform: none;
      opacity: 1;
    }
  }

  @keyframes -global-chirper-out-left {
    to {
      transform: translateX(-28%);
      opacity: 0.4;
    }
  }

  @keyframes -global-chirper-out-right {
    to {
      transform: translateX(28%);
      opacity: 0.4;
    }
  }

  /* macOS overlay titlebar: traffic lights take 28 px at the top and 78 px at the left. */
  :global(html[data-form='desktop'][data-platform='macos']) {
    --titlebar-inset: 28px;
    --banner-start-desktop: 78px;
  }

  /* In split view the traffic lights sit over the inbox pane only; the detail banner keeps the gutter. */
  .split .detail {
    --banner-start-desktop: initial;
  }
</style>
