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
  MediaFile,
  MediaKind,
  Message,
  LoginState,
  Network,
  SendResult,
  UnlockResult,
  AppLockState,
  AppPrefs,
  NotificationReply,
  PickedFile,
  PickKind,
} from './types';

/** Error returned by every core call. `code` is a stable machine string, `message` is for humans. */
export class CoreError extends Error {
  readonly code: string;

  constructor(code: string, message: string) {
    super(message);
    this.name = 'CoreError';
    this.code = code;
  }
}

/**
 * The UI's only view of the core. Methods mirror CONTRACT.md sections B and C: one method per
 * command, camelCase, taking one args object (the same keys as the contract's args JSON).
 * Methods without arguments take none. Every method rejects with a CoreError.
 */
export interface CoreApi {
  start(): Promise<void>;
  networks(): Promise<Network[]>;
  accounts(): Promise<Account[]>;

  startLogin(args: { network: string; flowId: string }): Promise<LoginState>;
  submitLoginInput(args: { processId: string; values: Record<string, string> }): Promise<LoginState>;
  submitLoginCookies(args: { processId: string; cookies: Record<string, string> }): Promise<LoginState>;
  waitLoginStep(args: { processId: string }): Promise<LoginState>;
  cancelLogin(args: { processId: string }): Promise<void>;
  logout(args: { network: string; loginId: string }): Promise<void>;

  listChats(args: { filter: ChatFilter }): Promise<Chat[]>;
  getChat(args: { roomId: string }): Promise<Chat | null>;
  listMessages(args: { roomId: string; beforeOrder: number; limit: number }): Promise<Message[]>;
  loadOlder(args: { roomId: string }): Promise<HasMore>;
  syncHistory(args: { network: string }): Promise<void>;
  markUnread(args: { roomId: string; unread: boolean }): Promise<void>;

  sendText(args: { roomId: string; text: string; replyTo: string; txnId: string }): Promise<SendResult>;
  sendMedia(args: {
    roomId: string;
    path: string;
    mime: string;
    fileName: string;
    caption: string;
    kind: MediaKind;
    replyTo: string;
    txnId: string;
  }): Promise<SendResult>;
  edit(args: { roomId: string; eventId: string; text: string }): Promise<void>;
  redact(args: { roomId: string; eventId: string }): Promise<void>;
  react(args: { roomId: string; eventId: string; key: string }): Promise<void>;
  unreact(args: { roomId: string; eventId: string; key: string }): Promise<void>;
  markRead(args: { roomId: string; eventId: string }): Promise<void>;
  setTyping(args: { roomId: string; typing: boolean }): Promise<void>;
  setChatFlags(args: { roomId: string; flags: ChatFlags }): Promise<void>;
  mediaFile(args: { url: string }): Promise<MediaFile>;
  search(args: { query: string; limit: number }): Promise<Message[]>;

  settings(): Promise<CoreSettings>;
  setSettings(args: { settings: CoreSettings }): Promise<void>;

  appInfo(): Promise<AppInfo>;
  haptic(args: { kind: HapticKind }): Promise<void>;
  setBadge(args: { count: number }): Promise<void>;
  notifyMessage(args: { roomId: string; title: string; body: string; avatarPath: string }): Promise<void>;
  lockStatus(): Promise<{ locked: boolean }>;
  unlock(): Promise<UnlockResult>;
  setSecureScreen(args: { enabled: boolean }): Promise<void>;
  openExternal(args: { url: string }): Promise<void>;
  getAppPrefs(): Promise<AppPrefs>;
  setAppPrefs(args: { prefs: AppPrefs }): Promise<void>;
  pickFiles(args: { kind: PickKind }): Promise<PickedFile[]>;

  /** Subscribes to core-batch events. Returns an unsubscribe function. */
  onEvents(cb: (events: CoreEvent[]) => void): () => void;
  /** app-lock: the app locked or unlocked. */
  onAppLock(cb: (state: AppLockState) => void): () => void;
  /** deep-link: chirper://chat/<roomId> and similar. */
  onDeepLink(cb: (url: string) => void): () => void;
  /** notification-reply: an inline reply typed in a notification. */
  onNotificationReply(cb: (reply: NotificationReply) => void): () => void;
}
