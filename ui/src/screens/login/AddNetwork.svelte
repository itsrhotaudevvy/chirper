<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import { core, type LoginState } from '../../lib/core';
  import { accounts } from '../../lib/stores';
  import { haptic } from '../../lib/haptics';
  import Banner from '../../lib/components/Banner.svelte';
  import Button from '../../lib/components/Button.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import { networkTiles, type NetworkTile } from './networkGrid';
  import { statusWord, viewForStep, type LoginView } from './loginView';
  import LoginDisplay from './LoginDisplay.svelte';
  import FlowForm from './FlowForm.svelte';
  import TextField from '../../lib/components/TextField.svelte';

  interface Props {
    /** Opens the flow of this network straight away (deep link or from an account). */
    network?: string;
    onclose: () => void;
  }

  let { network, onclose }: Props = $props();

  /** The 14 bright spectrum colours, in order, for the picker banner. */
  const RAINBOW = Array.from({ length: 14 }, (_, i) => `var(--banner-spectrum-${i})`);
  const NOT_SUPPORTED = 'Not supported yet on this platform';

  type Stage = 'grid' | 'flows' | 'login';

  let stage = $state<Stage>('grid');
  let chosen = $state<NetworkTile | null>(null);
  let view = $state<LoginView | null>(null);
  let busy = $state(false);
  let failed = $state(false);
  let cookieRefused = $state(false);
  let cookieError = $state('');
  let cookieToken = $state('');
  let showManualToken = $state(false);
  let finished = $state(false);
  let pulse = $state(false);
  let hint = $state('');

  // Not reactive: the login process and the attempt counter guard every async result.
  let processId = '';
  let flowId = '';
  let webviewLaunchedFor = '';
  let attempt = 0;
  let doneTimer: ReturnType<typeof setTimeout> | undefined;

  const tiles = $derived(networkTiles(accounts.networks));

  let autoStarted = false;
  $effect(() => {
    const id = network;
    if (autoStarted || !id) return;
    const tile = tiles.find((t) => t.id === id);
    if (!tile) return;
    autoStarted = true;
    untrack(() => choose(tile));
  });

  function choose(tile: NetworkTile): void {
    if (!tile.available) {
      hint = `${tile.name}: ${tile.reason || 'Unavailable'}`;
      return;
    }
    hint = '';
    chosen = tile;
    if (tile.flows.length === 0) {
      stage = 'login';
      view = { kind: 'unsupported' };
    } else if (tile.flows.length === 1 && tile.flows[0]) {
      void begin(tile, tile.flows[0].id);
    } else {
      stage = 'flows';
    }
  }

  async function begin(tile: NetworkTile, flow: string): Promise<void> {
    const current = ++attempt;
    dropProcess();
    chosen = tile;
    flowId = flow;
    stage = 'login';
    view = null;
    failed = false;
    cookieRefused = false;
    cookieError = '';
    cookieToken = '';
    showManualToken = false;
    webviewLaunchedFor = '';
    finished = false;
    busy = true;
    try {
      const state = await core.startLogin({ network: tile.id, flowId: flow });
      processId = state.processId;
      await present(state, current);
    } catch {
      if (current === attempt) fail();
    } finally {
      if (current === attempt) busy = false;
    }
  }

  /** Shows a step. Display steps are followed by waitLoginStep until the core moves on. */
  async function present(first: LoginState, current: number): Promise<void> {
    let state: LoginState | null = first;
    while (state && current === attempt) {
      const next: LoginView = viewForStep(state.step);
      view = next;
      if (next.kind === 'complete') {
        complete();
        return;
      }
      if (state.step.type !== 'display_and_wait') return;
      state = await core.waitLoginStep({ processId: state.processId });
    }
  }

  async function submit(values: Record<string, string>): Promise<void> {
    const current = attempt;
    busy = true;
    try {
      await present(await core.submitLoginInput({ processId, values }), current);
    } catch {
      if (current === attempt) fail();
    } finally {
      if (current === attempt) busy = false;
    }
  }

  async function continueCookies(token?: string): Promise<void> {
    const current = attempt;
    busy = true;
    cookieRefused = false;
    cookieError = '';
    try {
      let cookies: Record<string, string> = {};
      if (token) {
        const trimmed = token.trim();
        if (trimmed.startsWith('{') && trimmed.endsWith('}')) {
          try {
            cookies = JSON.parse(trimmed) as Record<string, string>;
          } catch {
            cookies['token'] = trimmed;
          }
        } else {
          cookies['token'] = trimmed;
        }
      }
      await present(await core.submitLoginCookies({ processId, cookies }), current);
    } catch (err: unknown) {
      if (current === attempt) {
        cookieRefused = true;
        cookieError =
          err instanceof Error
            ? err.message
            : err && typeof err === 'object' && 'message' in err && typeof err.message === 'string'
              ? err.message
              : 'Failed to sign in';
      }
    } finally {
      if (current === attempt) busy = false;
    }
  }

  async function launchWebviewLogin(cookiesView: Extract<LoginView, { kind: 'cookies' }>): Promise<void> {
    if (!cookiesView.url || busy) return;
    const current = attempt;
    busy = true;
    cookieRefused = false;
    cookieError = '';
    try {
      const res = await core.openLoginWebview({
        url: cookiesView.url,
        extractJs: cookiesView.extractJs,
        waitForPattern: cookiesView.waitForUrlPattern,
        userAgent: cookiesView.userAgent,
        title: chosen?.name ?? 'Sign In',
      });
      if (current === attempt && res.token) {
        cookieToken = res.token;
        await continueCookies(res.token);
      }
    } catch (err: unknown) {
      if (current === attempt) {
        cookieRefused = true;
        cookieError =
          err instanceof Error
            ? err.message
            : err && typeof err === 'object' && 'message' in err && typeof err.message === 'string'
              ? err.message
              : 'Could not open login webview';
      }
    } finally {
      if (current === attempt) busy = false;
    }
  }

  $effect(() => {
    if (view?.kind === 'cookies' && processId && webviewLaunchedFor !== processId) {
      webviewLaunchedFor = processId;
      const cv = view as Extract<LoginView, { kind: 'cookies' }>;
      untrack(() => void launchWebviewLogin(cv));
    }
  });

  function complete(): void {
    finished = true;
    pulse = true;
    haptic('success');
    doneTimer = setTimeout(onclose, 900);
  }

  function fail(): void {
    failed = true;
    haptic('warning');
  }

  function retry(): void {
    if (chosen) void begin(chosen, flowId);
  }

  async function cancel(): Promise<void> {
    const wasFinished = finished;
    const id = processId;
    attempt++;
    processId = '';
    busy = false;
    if (id && !wasFinished) await core.cancelLogin({ processId: id }).catch(() => undefined);
    if (network) onclose();
    else reset();
  }

  function back(): void {
    if (network) onclose();
    else reset();
  }

  function reset(): void {
    stage = 'grid';
    chosen = null;
    view = null;
    failed = false;
    finished = false;
  }

  /** Forgets the current process and cancels it in the core, unless it already completed. */
  function dropProcess(): void {
    const id = processId;
    processId = '';
    if (id && !finished) void core.cancelLogin({ processId: id }).catch(() => undefined);
  }

  onDestroy(() => {
    attempt++;
    if (doneTimer !== undefined) clearTimeout(doneTimer);
    dropProcess();
  });
</script>

<div class="screen">
  {#if stage === 'grid'}
    <Banner colors={RAINBOW} title="Add a network" action={{ label: 'Close', onclick: onclose }} />
    <div class="body">
      <div class="grid">
        {#each tiles as tile (tile.id)}
          <button
            class="tile"
            class:dim={!tile.available}
            type="button"
            style:--c={tile.colour}
            aria-label={tile.available ? tile.name : `${tile.name}, unavailable`}
            onclick={() => choose(tile)}
          >
            <span class="name">{tile.name}</span>
          </button>
        {/each}
      </div>
      {#if tiles.length === 0}
        <p class="t-body t-muted">No networks are available.</p>
      {/if}
      {#if hint}<StatusStrip tone="info" message={hint} />{/if}
    </div>
  {:else if stage === 'flows' && chosen}
    {@const net = chosen}
    <Banner colors={[net.colour]} logo={net.id} title={net.name} status="Choose" action={{ label: 'Back', onclick: back }} />
    <div class="body">
      {#each net.flows as flow (flow.id)}
        <div class="flow">
          <Button color={net.id} onclick={() => void begin(net, flow.id)}>{flow.name}</Button>
          {#if flow.description}<p class="t-label t-muted">{flow.description}</p>{/if}
        </div>
      {/each}
    </div>
  {:else if chosen}
    {@const net = chosen}
    <div class="banner-wrap" class:pulse>
      {#if failed}
        <Banner colors={['var(--banner-danger)']} title="Failed" action={{ label: 'Retry', onclick: retry }} />
      {:else if finished}
        <Banner colors={[net.colour]} logo={net.id} title={net.name} status="Connected" />
      {:else}
        <Banner
          colors={[net.colour]}
          logo={net.id}
          title={net.name}
          status={view ? statusWord(view) : 'Starting'}
          action={{ label: 'Cancel', onclick: () => void cancel() }}
        />
      {/if}
    </div>
    <div class="body">
      {#if failed}
        <div class="actions">
          <Button variant="quiet" onclick={back}>Close</Button>
        </div>
      {:else if view?.kind === 'form'}
        <p class="t-body">{view.instructions}</p>
        {#key view.stepId}
          <FlowForm fields={view.fields} network={net.id} {busy} onsubmit={(values) => void submit(values)} />
        {/key}
      {:else if view?.kind === 'qr' || view?.kind === 'code'}
        <p class="t-body">{view.instructions}</p>
        <LoginDisplay {view} colour={net.colour} />
      {:else if view?.kind === 'waiting'}
        <p class="t-body">{view.instructions}</p>
      {:else if view?.kind === 'cookies'}
        {@const cookiesView = view}
        <p class="t-body">{cookiesView.instructions || `Sign in with ${net.name} to connect your account.`}</p>
        <div class="flow-actions">
          <Button color={net.id} disabled={busy} onclick={() => void launchWebviewLogin(cookiesView)}>
            {busy ? 'Signing in…' : `Sign in with ${net.name}`}
          </Button>
        </div>
        {#if showManualToken}
          <div class="manual-token">
            <TextField
              label={net.id === 'discord' ? 'Discord token' : `${net.name} token / cookies`}
              type="password"
              placeholder={net.id === 'discord' ? 'Paste Discord token' : `Paste ${net.name} token or cookies`}
              bind:value={cookieToken}
              disabled={busy}
              onsubmit={() => void continueCookies(cookieToken.trim())}
            />
            <p class="t-label t-muted">In browser: open Developer Tools (Network or Storage) to find your token or cookies.</p>
            <div class="actions">
              <Button color={net.id} disabled={busy || !cookieToken.trim()} onclick={() => void continueCookies(cookieToken.trim())}>Continue</Button>
            </div>
          </div>
        {:else}
          <div class="manual-toggle">
            <Button variant="quiet" onclick={() => (showManualToken = true)}>Or enter {net.id === 'discord' ? 'token' : 'credentials'} manually</Button>
          </div>
        {/if}
        {#if cookieRefused}<StatusStrip tone="warn" message={cookieError || NOT_SUPPORTED} />{/if}
      {:else if view?.kind === 'unsupported'}
        <StatusStrip tone="warn" message={NOT_SUPPORTED} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .screen {
    height: 100%;
    overflow-y: auto;
    background: var(--bg);
    padding-bottom: calc(var(--safe-bottom) + var(--s-8));
  }

  .body {
    display: grid;
    gap: var(--s-4);
    max-width: 640px;
    margin: 0 auto;
    padding: var(--s-4) var(--gutter) 0;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--s-2);
  }

  .tile {
    position: relative;
    display: flex;
    align-items: flex-end;
    min-height: 96px;
    padding: var(--s-3);
    border-radius: var(--r-lg);
    background: var(--c);
    color: var(--on-banner);
    text-align: start;
    transition:
      transform var(--dur-quick) var(--ease-out),
      opacity var(--dur-base) var(--ease-out);
  }

  .tile:active {
    transform: scale(0.97);
  }

  .tile.dim {
    opacity: 0.35;
  }

  .name {
    font-size: var(--t-headline-size);
    line-height: 1.2;
    font-weight: 700;
    overflow-wrap: anywhere;
  }

  .flow {
    display: grid;
    gap: var(--s-2);
  }

  .flow :global(.btn) {
    width: 100%;
    min-height: 56px;
    font-size: var(--t-headline-size);
  }

  .flow-actions :global(.btn) {
    width: 100%;
    min-height: 52px;
    font-size: var(--t-headline-size);
  }

  .manual-toggle {
    display: flex;
    justify-content: center;
  }

  .manual-token {
    display: grid;
    gap: var(--s-3);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }

  .banner-wrap.pulse {
    animation: pulse-once var(--dur-page) var(--ease-out);
  }

  @keyframes pulse-once {
    40% {
      transform: scale(1.03);
    }
  }

  @media (min-width: 600px) {
    .grid {
      grid-template-columns: repeat(3, minmax(0, 1fr));
    }
  }

  @media (min-width: 900px) {
    .grid {
      grid-template-columns: repeat(4, minmax(0, 1fr));
    }
  }
</style>
