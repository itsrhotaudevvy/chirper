import { isTauri } from './core';

/**
 * URL for a Matrix content URI (mxc://...). Inside Tauri it is served by the chirper-media protocol
 * (resolved in Rust, only for paths inside the data dir). In a browser there is no media server,
 * so it returns null and callers show a placeholder.
 */
export function mediaUrl(mxc: string): string | null {
  if (!mxc.startsWith('mxc://')) return null;
  if (!isTauri()) return null;
  return `chirper-media://localhost/${encodeURIComponent(mxc)}`;
}
