<script lang="ts">
  import { onMount } from 'svelte';
  import { core, type Account, type AppInfo } from '../../lib/core';
  import { SETTINGS_SLOTS } from '../../lib/spectrum';
  import { accountKey } from '../../lib/stores/accounts.svelte';
  import { accounts, settings } from '../../lib/stores';
  import Banner from '../../lib/components/Banner.svelte';
  import AccountPage from './AccountPage.svelte';
  import AboutSection from './AboutSection.svelte';
  import AccountsSection from './AccountsSection.svelte';
  import AdvancedSection from './AdvancedSection.svelte';
  import AppearanceSection from './AppearanceSection.svelte';
  import ChatsSection from './ChatsSection.svelte';
  import NotificationsSection from './NotificationsSection.svelte';
  import PrivacySection from './PrivacySection.svelte';
  import SettingsSection from './SettingsSection.svelte';
  import SyncSection from './SyncSection.svelte';
  import { router } from '../../lib/router.svelte';

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  type SectionId = 'accounts' | 'notifications' | 'privacy' | 'appearance' | 'chats' | 'sync' | 'advanced' | 'about';

  /** One section is open at a time; Accounts starts open so the first screen is useful. */
  let open = $state<SectionId | null>('accounts');
  let detail = $state<string | null>(null);
  let info = $state<AppInfo | null>(null);

  const selected = $derived<Account | null>(accounts.sortedAccounts.find((a) => accountKey(a) === detail) ?? null);

  /** The five category colours, in spectrum order, blended into the page banner (bright, every theme). */
  const bannerColours = [...Object.values(SETTINGS_SLOTS)]
    .sort((a, b) => a - b)
    .map((slot) => `var(--banner-spectrum-${slot})`);

  const THEME_LABEL = { system: 'System', dark: 'Dark', light: 'Light', amoled: 'AMOLED' } as const;

  const desktop = $derived(info?.formFactor === 'desktop');
  const accountCount = $derived(accounts.list.length);

  onMount(() => {
    void accounts.load();
    core
      .appInfo()
      .then((value) => (info = value))
      .catch(() => (info = null));
  });

  function toggle(id: SectionId): void {
    open = open === id ? null : id;
  }
</script>

<div class="page">
  {#if selected}
    {#key accountKey(selected)}
      <AccountPage account={selected} onback={() => (detail = null)} />
    {/key}
  {:else}
    <Banner colors={bannerColours} title="Settings" action={{ label: 'Done', onclick: onclose }} />

    <div class="column">
      <SettingsSection
        id="accounts"
        color="var(--cat-accounts)"
        title="Accounts"
        summary={accountCount === 1 ? '1 account' : `${accountCount} accounts`}
        open={open === 'accounts'}
        ontoggle={() => toggle('accounts')}
      >
        <AccountsSection onopen={(a) => (detail = accountKey(a))} onadd={() => router.go('add-network')} />
      </SettingsSection>

      <SettingsSection
        id="notifications"
        color="var(--cat-notifications)"
        title="Notifications"
        summary={settings.app.notifications ? 'On' : 'Off'}
        open={open === 'notifications'}
        ontoggle={() => toggle('notifications')}
      >
        <NotificationsSection />
      </SettingsSection>

      <SettingsSection
        id="privacy"
        color="var(--cat-privacy)"
        title="Privacy & security"
        summary={settings.app.appLock ? 'Lock on' : 'Lock off'}
        open={open === 'privacy'}
        ontoggle={() => toggle('privacy')}
      >
        <PrivacySection />
      </SettingsSection>

      <SettingsSection
        id="appearance"
        color="var(--cat-appearance)"
        title="Appearance"
        summary={THEME_LABEL[settings.theme]}
        open={open === 'appearance'}
        ontoggle={() => toggle('appearance')}
      >
        <AppearanceSection />
      </SettingsSection>

      <SettingsSection
        id="chats"
        color="var(--cat-chats)"
        title="Chats & storage"
        open={open === 'chats'}
        ontoggle={() => toggle('chats')}
      >
        <ChatsSection {desktop} />
      </SettingsSection>

      <SettingsSection
        id="sync"
        color="var(--cat-appearance)"
        title="Cross-platform sync"
        summary="Serverless sync"
        open={open === 'sync'}
        ontoggle={() => toggle('sync')}
      >
        <SyncSection />
      </SettingsSection>

      <SettingsSection
        id="advanced"
        color="var(--cat-about)"
        title="Advanced"
        summary="Telegram API keys"
        open={open === 'advanced'}
        ontoggle={() => toggle('advanced')}
      >
        <AdvancedSection />
      </SettingsSection>

      <SettingsSection id="about" color="var(--cat-about)" title="About" open={open === 'about'} ontoggle={() => toggle('about')}>
        <AboutSection {info} />
      </SettingsSection>
    </div>
  {/if}
</div>

<style>
  .page {
    height: 100%;
    overflow-y: auto;
    background: var(--bg);
    padding-bottom: var(--safe-bottom);
  }

  .column {
    display: grid;
    gap: var(--s-3);
    max-width: 640px;
    margin: 0 auto;
    padding: var(--s-4) var(--gutter) calc(var(--safe-bottom) + var(--s-8));
  }
</style>
