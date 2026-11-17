import { defineConfig, type Plugin } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const host = process.env['TAURI_DEV_HOST'];
const platform = process.env['TAURI_ENV_PLATFORM'];
const debug = Boolean(process.env['TAURI_ENV_DEBUG']);

// The browser build (GitHub Pages): fully static, in-memory demo core, and a Content-Security-Policy
// that forbids every network connection. Selected with CHIRPER_WEB=1; the desktop and mobile builds
// are unaffected.
const web = process.env['CHIRPER_WEB'] === '1';
const webBase = process.env['CHIRPER_WEB_BASE'] ?? '/';

const WEB_CSP = [
  "default-src 'none'",
  "script-src 'self'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob:",
  "font-src 'self' data:",
  "connect-src 'none'",
  "media-src 'self' data: blob:",
  "worker-src 'none'",
  "frame-src 'none'",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'none'",
].join('; ');

const webCsp: Plugin = {
  name: 'chirper-web-csp',
  transformIndexHtml: {
    order: 'pre',
    handler: (html) => html.replace('<head>', `<head>\n    <meta http-equiv="Content-Security-Policy" content="${WEB_CSP}" />`),
  },
};

export default defineConfig({
  base: web ? webBase : '/',
  plugins: [svelte(), ...(web ? [webCsp] : [])],
  // Under vitest, resolve Svelte's client build so components can be mounted in happy-dom.
  resolve: process.env['VITEST'] ? { conditions: ['browser'] } : {},
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    ...(host ? { hmr: { protocol: 'ws', host, port: 1421 } } : {}),
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: platform === 'macos' || platform === 'ios' ? 'safari15' : 'chrome105',
    minify: debug ? false : 'oxc',
    sourcemap: debug,
    outDir: web ? 'dist-web' : 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
