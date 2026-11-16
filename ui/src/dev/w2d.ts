/*
 * Dev-only harness for W2d screens (settings, add network, onboarding, lock). Served at
 * /src/dev/w2d.html in `pnpm dev`; not part of the build.
 *   ?screen=settings|add|onboarding|lock   ?network=<id>   ?theme=dark|light|amoled
 *   &tap=<css selector>  (clicked 300 ms after mount, repeatable) to open a section or row
 */
import '../styles/tokens.css';
import '../styles/base.css';
import { mount } from 'svelte';
import { connectStores } from '../lib/stores';
import { core } from '../lib/core';
import SettingsScreen from '../screens/settings/SettingsScreen.svelte';
import AddNetwork from '../screens/login/AddNetwork.svelte';
import Onboarding from '../screens/onboarding/Onboarding.svelte';
import LockScreen from '../screens/lock/LockScreen.svelte';

const params = new URLSearchParams(location.search);
const theme = params.get('theme');
if (theme === 'dark' || theme === 'light' || theme === 'amoled') {
  document.documentElement.setAttribute('data-theme', theme);
}

const target = document.getElementById('app');
if (!target) throw new Error('missing #app');
const frame = document.createElement('div');
frame.style.cssText = 'position:fixed;inset:0;background:var(--bg);color:var(--on);overflow:hidden';
target.appendChild(frame);

await connectStores();
// ?hold=1 keeps a login step open (the fake core completes display steps at once) so it can be shot.
if (params.get('hold') === '1') core.waitLoginStep = () => new Promise(() => undefined);
// ?fail=1 makes every login start fail, to see the error banner.
if (params.get('fail') === '1') core.startLogin = () => Promise.reject(new Error('offline'));

const noop = (): void => undefined;
switch (params.get('screen')) {
  case 'add':
    mount(AddNetwork, { target: frame, props: { network: params.get('network') ?? undefined, onclose: noop } });
    break;
  case 'onboarding':
    mount(Onboarding, { target: frame, props: { ondone: noop } });
    break;
  case 'lock':
    mount(LockScreen, { target: frame, props: {} });
    break;
  default:
    mount(SettingsScreen, { target: frame, props: { onclose: noop } });
}

for (const [i, selector] of params.getAll('tap').entries()) {
  setTimeout(() => document.querySelector<HTMLElement>(selector)?.click(), 300 + i * 300);
}
