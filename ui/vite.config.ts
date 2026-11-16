import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

const host = process.env.TAURI_DEV_HOST;
const platform = process.env.TAURI_ENV_PLATFORM;
const debug = Boolean(process.env.TAURI_ENV_DEBUG);

export default defineConfig({
  plugins: [svelte()],
  // Under vitest, resolve Svelte's client build so components can be mounted in happy-dom.
  resolve: process.env.VITEST ? { conditions: ['browser'] } : {},
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: platform === 'macos' || platform === 'ios' ? 'safari15' : 'chrome105',
    minify: debug ? false : 'oxc',
    sourcemap: debug,
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
