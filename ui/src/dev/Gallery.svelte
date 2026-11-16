<script lang="ts">
  /* Placeholder component gallery (W1c). The real shell replaces this file in W2b. */
  import { onMount } from 'svelte';
  import type { Chat, Message } from '../lib/core';
  import { connectStores, chats, messages, settings, ui, type ThemePref } from '../lib/stores';
  import { SPECTRUM_DARK, bannerCss, networkCss, ACCENT_PRESETS, type AccentId } from '../lib/spectrum';
  import Avatar from '../lib/components/Avatar.svelte';
  import Banner from '../lib/components/Banner.svelte';
  import Button from '../lib/components/Button.svelte';
  import IconButton from '../lib/components/IconButton.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import { ICONS, type IconName } from '../lib/components/icons';
  import SectionBanner from '../lib/components/SectionBanner.svelte';
  import Segmented from '../lib/components/Segmented.svelte';
  import Sheet from '../lib/components/Sheet.svelte';
  import Skeleton from '../lib/components/Skeleton.svelte';
  import StatusStrip from '../lib/components/StatusStrip.svelte';
  import TextField from '../lib/components/TextField.svelte';
  import Toggle from '../lib/components/Toggle.svelte';
  import TypingDots from '../lib/components/TypingDots.svelte';
  import UnreadPill from '../lib/components/UnreadPill.svelte';
  import VirtualList from '../lib/components/VirtualList.svelte';

  let error = $state('');
  let sheetOpen = $state(false);
  let name = $state('');
  let themeChoice = $state<ThemePref>(settings.theme);
  const themes = [
    { value: 'system', label: 'System' },
    { value: 'dark', label: 'Dark' },
    { value: 'light', label: 'Light' },
    { value: 'amoled', label: 'AMOLED' },
  ] as const;

  const shown = $derived(chats.active.slice(0, 8));
  const openRoom = '!whatsapp.family:chirper.local';
  const familyMessages = $derived(messages.timeline(openRoom).items);

  onMount(() => {
    const disposeUi = ui.attach();
    let disposeStores: (() => void) | undefined;
    connectStores()
      .then((dispose) => {
        disposeStores = dispose;
        return messages.open(openRoom);
      })
      .catch((err: unknown) => {
        error = err instanceof Error ? err.message : 'Could not start the demo core';
      });
    return () => {
      disposeUi();
      disposeStores?.();
    };
  });

  function cycleAccent(): void {
    const ids = ACCENT_PRESETS.map((a) => a.id);
    const next = ids[(ids.indexOf(settings.accent) + 1) % ids.length] as AccentId;
    settings.updateLocal({ accent: next });
  }

  function keyOfMessage(m: Message): string {
    return m.eventId;
  }

  function networkOf(chat: Chat): string {
    return chat.network;
  }
</script>

<main class="gallery">
  <Banner
    colors={[networkCss('whatsapp'), networkCss('telegram')]}
    title="Components"
    status="Demo core, W1c gallery"
    action={{ label: 'Accent', onclick: cycleAccent }}
  />

  <div class="page">
    {#if error}
      <StatusStrip tone="danger" message={error} />
    {/if}

    <section class="block" aria-labelledby="h-rainbow">
      <h2 id="h-rainbow" class="t-title">Rainbow</h2>
      <div class="swatches">
        {#each Array.from({ length: SPECTRUM_DARK.length }, (_, i) => i) as i (i)}
          <span class="swatch" style:background={`var(--banner-spectrum-${i})`} title={`slot ${i}`}></span>
        {/each}
      </div>
    </section>

    <section class="block" aria-labelledby="h-chats">
      <h2 id="h-chats" class="t-title">Chats</h2>
      <ul class="rows">
        {#each shown as chat (chat.roomId)}
          <li class="chat-row">
            <Avatar name={chat.name} id={chat.roomId} network={networkOf(chat)} />
            <div class="chat-text">
              <span class="t-headline truncate">{chat.name}</span>
              <span class="t-body t-muted truncate">{chat.preview?.body ?? ''}</span>
            </div>
            <UnreadPill count={chat.unreadCount} network={networkOf(chat)} />
          </li>
        {/each}
      </ul>
      <SectionBanner color={bannerCss('signal')} label="Signal" />
      <SectionBanner color={bannerCss('slack')} label="Slack" />
    </section>

    <section class="block" aria-labelledby="h-timeline">
      <h2 id="h-timeline" class="t-title">Timeline (virtual, reverse)</h2>
      <div class="timeline">
        <VirtualList items={familyMessages} keyOf={keyOfMessage} label="Family messages" reverse estimate={64} onStart={() => messages.loadOlder(openRoom)}>
          {#snippet row(m: Message)}
            <p class="bubble t-body selectable">
              <span class="who t-label">{m.fromMe ? 'You' : m.senderName}</span>
              {m.body || `[${m.type}]`}
            </p>
          {/snippet}
        </VirtualList>
      </div>
    </section>

    <section class="block" aria-labelledby="h-controls">
      <h2 id="h-controls" class="t-title">Controls</h2>
      <div class="row-wrap">
        <Button>Primary</Button>
        <Button variant="quiet">Quiet</Button>
        <Button color="telegram">Telegram</Button>
        <IconButton icon="send" label="Send" filled />
        <IconButton icon="search" label="Search" />
        <TypingDots label="Sam is typing" />
        <Skeleton width="120px" height="14px" />
      </div>
      <Segmented options={themes} bind:value={themeChoice} label="Theme" onchange={(v) => settings.updateLocal({ theme: v })} />
      <Toggle label="Hide notification content" description="Show only the sender" checked={settings.app.hideNotificationContent} onchange={(v) => settings.updateApp({ hideNotificationContent: v }).catch(() => undefined)} />
      <TextField label="Display name" bind:value={name} hint={name.length > 20 ? 'Keep it under 20 characters' : 'Shown to your contacts'} invalid={name.length > 20} />
      <StatusStrip tone="warn" message="Instagram needs you to log in again" action={{ label: 'Reconnect', onclick: () => undefined }} />
      <Button variant="quiet" onclick={() => (sheetOpen = true)}>Open sheet</Button>
    </section>

    <section class="block" aria-labelledby="h-icons">
      <h2 id="h-icons" class="t-title">Icons</h2>
      <div class="icons">
        {#each Object.keys(ICONS) as icon (icon)}
          <span class="icon-cell"><Icon name={icon as IconName} /><span class="t-label t-muted">{icon}</span></span>
        {/each}
      </div>
    </section>
  </div>

  <Sheet open={sheetOpen} title="Sheet" onclose={() => (sheetOpen = false)}>
    <p class="t-body">Bottom sheet on phones, centred panel on desktop. Escape or drag down to close.</p>
    <Button onclick={() => (sheetOpen = false)}>Done</Button>
  </Sheet>
</main>

<style>
  .gallery {
    min-height: 100dvh;
    padding-bottom: calc(var(--safe-bottom) + var(--s-8));
  }

  .page {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-8);
    padding: var(--s-4) var(--gutter) 0;
    max-width: 960px;
    margin-inline: auto;
  }

  .block {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: var(--s-4);
  }

  .swatches {
    display: grid;
    grid-template-columns: repeat(14, minmax(0, 1fr));
    gap: 4px;
  }

  .swatch {
    height: 28px;
    border-radius: var(--r-xs);
  }

  .rows {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }

  .chat-row {
    display: flex;
    align-items: center;
    gap: var(--s-3);
    min-height: var(--row-h);
  }

  .chat-text {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    flex: 1;
    min-width: 0;
  }

  .timeline {
    height: 320px;
    border-radius: var(--r-lg);
    background: var(--surface);
  }

  .bubble {
    display: grid;
    gap: 2px;
    padding: var(--s-1) var(--gutter);
  }

  .who {
    color: var(--on-muted);
  }

  .row-wrap {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--s-3);
  }

  .icons {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(96px, 1fr));
    gap: var(--s-3);
  }

  .icon-cell {
    display: grid;
    justify-items: center;
    gap: var(--s-1);
    padding: var(--s-2);
    border-radius: var(--r-md);
    background: var(--surface);
  }
</style>
