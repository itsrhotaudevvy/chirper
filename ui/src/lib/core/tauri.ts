import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { CoreError, type CoreApi } from './api';
import type {
  Account,
  AppInfo,
  Chat,
  ChatFilter,
  ChatFlags,
  CoreEvent,
  CoreSettings,
  HapticKind,
  HasMore,
  LoginState,
  MediaFile,
  MediaKind,
  Message,
  Network,
  SendResult,
  UnlockResult,
  AppLockState,
  AppPrefs,
  NotificationReply,
  PickedFile,
  PickKind,
  LoginWebviewResult,
} from './types';

/** Turns whatever Tauri rejected with into a CoreError. Rust rejects with { code, message }. */
export function normaliseError(err: unknown): CoreError {
  if (err instanceof CoreError) return err;
  if (typeof err === 'object' && err !== null) {
    const { code, message } = err as { code?: unknown; message?: unknown };
    if (typeof code === 'string' && typeof message === 'string') return new CoreError(code, message);
    if (typeof message === 'string') return new CoreError('unknown', message);
  }
  if (typeof err === 'string') return new CoreError('unknown', err);
  return new CoreError('unknown', 'Unexpected error');
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (err) {
    throw normaliseError(err);
  }
}

/** CoreApi backed by Tauri commands (CONTRACT.md section C) and the core-batch event. */
export class TauriCore implements CoreApi {
  start(): Promise<void> {
    return call('start');
  }
  networks(): Promise<Network[]> {
    return call('networks');
  }
  accounts(): Promise<Account[]> {
    return call('accounts');
  }

  startLogin(args: { network: string; flowId: string }): Promise<LoginState> {
    return call('start_login', { ...args });
  }
  submitLoginInput(args: { processId: string; values: Record<string, string> }): Promise<LoginState> {
    return call('submit_login_input', { ...args });
  }
  submitLoginCookies(args: { processId: string; cookies: Record<string, string> }): Promise<LoginState> {
    return call('submit_login_cookies', { ...args });
  }
  waitLoginStep(args: { processId: string }): Promise<LoginState> {
    return call('wait_login_step', { ...args });
  }
  cancelLogin(args: { processId: string }): Promise<void> {
    return call('cancel_login', { ...args });
  }
  logout(args: { network: string; loginId: string }): Promise<void> {
    return call('logout', { ...args });
  }

  listChats(args: { filter: ChatFilter }): Promise<Chat[]> {
    return call('list_chats', { ...args });
  }
  getChat(args: { roomId: string }): Promise<Chat | null> {
    return call('get_chat', { ...args });
  }
  listMessages(args: { roomId: string; beforeOrder: number; limit: number }): Promise<Message[]> {
    return call('list_messages', { ...args });
  }
  loadOlder(args: { roomId: string }): Promise<HasMore> {
    return call('load_older', { ...args });
  }
  syncHistory(args: { network: string }): Promise<void> {
    return call('sync_history', { ...args });
  }
  markUnread(args: { roomId: string; unread: boolean }): Promise<void> {
    return call('mark_unread', { ...args });
  }

  sendText(args: { roomId: string; text: string; replyTo: string; txnId: string }): Promise<SendResult> {
    return call('send_text', { ...args });
  }
  sendMedia(args: {
    roomId: string;
    path: string;
    mime: string;
    fileName: string;
    caption: string;
    kind: MediaKind;
    replyTo: string;
    txnId: string;
  }): Promise<SendResult> {
    return call('send_media', { ...args });
  }
  edit(args: { roomId: string; eventId: string; text: string }): Promise<void> {
    return call('edit', { ...args });
  }
  redact(args: { roomId: string; eventId: string }): Promise<void> {
    return call('redact', { ...args });
  }
  react(args: { roomId: string; eventId: string; key: string }): Promise<void> {
    return call('react', { ...args });
  }
  unreact(args: { roomId: string; eventId: string; key: string }): Promise<void> {
    return call('unreact', { ...args });
  }
  markRead(args: { roomId: string; eventId: string }): Promise<void> {
    return call('mark_read', { ...args });
  }
  setTyping(args: { roomId: string; typing: boolean }): Promise<void> {
    return call('set_typing', { ...args });
  }
  setChatFlags(args: { roomId: string; flags: ChatFlags }): Promise<void> {
    return call('set_chat_flags', { ...args });
  }
  mediaFile(args: { url: string }): Promise<MediaFile> {
    return call('media_file', { ...args });
  }
  search(args: { query: string; limit: number }): Promise<Message[]> {
    return call('search', { ...args });
  }

  settings(): Promise<CoreSettings> {
    return call('settings');
  }
  setSettings(args: { settings: CoreSettings }): Promise<void> {
    return call('set_settings', { ...args });
  }

  appInfo(): Promise<AppInfo> {
    return call('app_info');
  }
  haptic(args: { kind: HapticKind }): Promise<void> {
    return call('haptic', { ...args });
  }
  setBadge(args: { count: number }): Promise<void> {
    return call('set_badge', { ...args });
  }
  notifyMessage(args: { roomId: string; title: string; body: string; avatarPath: string }): Promise<void> {
    return call('notify_message', { ...args });
  }
  lockStatus(): Promise<{ locked: boolean }> {
    return call('lock_status');
  }
  unlock(): Promise<UnlockResult> {
    return call('unlock');
  }
  setSecureScreen(args: { enabled: boolean }): Promise<void> {
    return call('set_secure_screen', { ...args });
  }
  openExternal(args: { url: string }): Promise<void> {
    return call('open_external', { ...args });
  }
  openLoginWebview(args: {
    url: string;
    extractJs?: string;
    waitForPattern?: string;
    userAgent?: string;
    title?: string;
  }): Promise<LoginWebviewResult> {
    return call('open_login_webview', { ...args });
  }
  getAppPrefs(): Promise<AppPrefs> {
    return call('get_app_prefs');
  }
  setAppPrefs(args: { prefs: AppPrefs }): Promise<void> {
    return call('set_app_prefs', { ...args });
  }
  pickFiles(args: { kind: PickKind }): Promise<PickedFile[]> {
    return call('pick_files', { ...args });
  }

  onEvents(cb: (events: CoreEvent[]) => void): () => void {
    return subscribe<CoreEvent[]>('core-batch', cb);
  }
  onAppLock(cb: (state: AppLockState) => void): () => void {
    return subscribe<AppLockState>('app-lock', cb);
  }
  onDeepLink(cb: (url: string) => void): () => void {
    return subscribe<{ url: string }>('deep-link', (payload) => cb(payload.url));
  }
  onNotificationReply(cb: (reply: NotificationReply) => void): () => void {
    return subscribe<NotificationReply>('notification-reply', cb);
  }
}

/** Listens to a Tauri event. The returned function unsubscribes, even if listen() is still pending. */
function subscribe<T>(event: string, cb: (payload: T) => void): () => void {
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  listen<T>(event, (e) => {
    if (!disposed) cb(e.payload);
  })
    .then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    unlisten?.();
  };
}
