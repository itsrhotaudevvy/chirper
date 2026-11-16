import './styles/tokens.css';
import './styles/base.css';
import { mount } from 'svelte';
import App from './App.svelte';
import { settings } from './lib/stores/settings.svelte';

settings.applyLocal();
// Dev-only theme override for screenshot checks: ?theme=dark|light|amoled
const devTheme = import.meta.env.DEV ? new URLSearchParams(location.search).get('theme') : null;
if (devTheme === 'dark' || devTheme === 'light' || devTheme === 'amoled') {
  document.documentElement.setAttribute('data-theme', devTheme);
}
const target = document.getElementById('app');
if (!target) throw new Error('missing #app element');
// Dev-only component gallery: http://localhost:1420/?gallery
if (import.meta.env.DEV && new URLSearchParams(location.search).has('gallery')) {
  void import('./dev/Gallery.svelte').then(({ default: Gallery }) => mount(Gallery, { target }));
} else {
  mount(App, { target });
}
