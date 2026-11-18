import { isTauri } from './core';

/**
 * Tauri serves custom schemes as `http://<scheme>.localhost/` on Windows and Android, and as
 * `<scheme>://localhost/` elsewhere. A URL in the wrong form never loads, so the platform decides.
 */
function chirperMediaBase(): string {
  const webviewUsesHttp = typeof navigator !== 'undefined' && /Android|Windows/i.test(navigator.userAgent);
  return webviewUsesHttp ? 'http://chirper-media.localhost' : 'chirper-media://localhost';
}

/**
 * URL for a Matrix content URI (mxc://...). Inside Tauri it is served by the chirper-media protocol
 * (resolved in Rust, only for paths inside the data dir). In a browser there is no media server,
 * so it returns null and callers show a placeholder.
 */
export function mediaUrl(mxc: string): string | null {
  if (mxc.startsWith('http://') || mxc.startsWith('https://')) return mxc;
  if (!mxc.startsWith('mxc://')) return null;
  if (!isTauri()) return null;
  return `${chirperMediaBase()}/${encodeURIComponent(mxc)}`;
}
