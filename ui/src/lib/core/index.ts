import type { CoreApi } from './api';
import { FakeCore } from './fake';
import { TauriCore } from './tauri';

export { CoreError, type CoreApi } from './api';
export * from './types';

/** True inside the Tauri webview, false in a plain browser (pnpm dev). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** The core the UI talks to: Tauri commands in the app, an in-memory demo world in a browser. */
export const core: CoreApi = isTauri() ? new TauriCore() : new FakeCore();
