/**
 * TypeScript mirrors of the JSON model (docs/CONTRACT.md section A, authority go/core/engine and
 * go/core/store) and of the command argument types (section C). Field names are camelCase.
 */

export interface LoginFlow {
  id: string;
  name: string;
  description: string;
}

export interface Capabilities {
  edits: boolean;
  reactions: boolean;
  replies: boolean;
  redactions: boolean;
  typing: boolean;
  readReceipts: boolean;
  stickers: boolean;
  voice: boolean;
}

export interface Network {
  id: string;
  name: string;
  /** Brand colour from the connector. Ignored for colour coding (see spectrum.ts). */
  color: string;
  beta: boolean;
  loginFlows: LoginFlow[];
  capabilities: Capabilities;
  available: boolean;
  reason: string;
}

export type AccountState =
  | 'CONNECTED'
  | 'CONNECTING'
  | 'BAD_CREDENTIALS'
  | 'TRANSIENT_DISCONNECT'
  | 'UNKNOWN_ERROR'
  | 'LOGGED_OUT';

export interface Account {
  network: string;
  loginId: string;
  name: string;
  avatarUrl: string;
  state: AccountState;
  error: string;
  message: string;
}

export interface InputField {
  id: string;
  name: string;
  /** Free-form field kind from the connector (for example "phone", "code", "password", "token"). */
  type: string;
  description: string;
  options: string[];
  pattern: string;
}

export interface CookieSource {
  type: string;
  name: string;
  cookieDomain: string;
}

export interface CookieField {
  id: string;
  required: boolean;
  sources: CookieSource[];
}

export interface CookiesStep {
  url: string;
  userAgent: string;
  fields: CookieField[];
  extractJs: string;
  waitForUrlPattern?: string | undefined;
}

export interface LoginWebviewResult {
  token?: string;
  cancelled: boolean;
}

export type DisplayType = 'qr' | 'emoji' | 'code' | 'nothing';

export interface DisplayStep {
  type: DisplayType;
  data: string;
  imageUrl: string;
}

export type LoginStepType = 'user_input' | 'cookies' | 'display_and_wait' | 'complete';

export interface LoginStep {
  type: LoginStepType;
  stepId: string;
  instructions: string;
  userInput?: { fields: InputField[] };
  cookies?: CookiesStep;
  displayAndWait?: DisplayStep;
  complete?: { loginId: string };
}

export interface LoginState {
  processId: string;
  step: LoginStep;
}

export interface ChatPreview {
  sender: string;
  body: string;
  type: string;
}

export interface Chat {
  roomId: string;
  network: string;
  loginId: string;
  name: string;
  avatarUrl: string;
  topic: string;
  isDm: boolean;
  isGroup: boolean;
  isSpace: boolean;
  memberCount: number;
  /** Unix milliseconds. */
  lastActivity: number;
  preview: ChatPreview | null;
  unreadCount: number;
  mentionCount: number;
  pinned: boolean;
  archived: boolean;
  /** Unix milliseconds; 0 when not muted. */
  mutedUntil: number;
  typing: string[];
  parentId?: string;
  serverName?: string;
  isChannel?: boolean;
}

export interface MessageMedia {
  url: string;
  mime: string;
  size: number;
  width: number;
  height: number;
  durationMs: number;
  fileName: string;
  thumbnailUrl: string;
  blurhash: string;
}

export interface MessageReply {
  eventId: string;
  senderName: string;
  body: string;
}

export interface MessageReaction {
  key: string;
  count: number;
  fromMe: boolean;
  senders: string[];
}

export type MessageStatus = 'sending' | 'sent' | 'delivered' | 'read' | 'failed';

export interface Message {
  eventId: string;
  roomId: string;
  /** Monotonic ordering key within a room (larger is newer). */
  order: number;
  sender: string;
  senderName: string;
  senderAvatar: string;
  fromMe: boolean;
  /** Unix milliseconds. */
  ts: number;
  /** text | image | video | audio | voice | file | sticker (connector-defined values may extend this). */
  type: string;
  body: string;
  formattedBody: string;
  media: MessageMedia | null;
  replyTo: MessageReply | null;
  edited: boolean;
  redacted: boolean;
  reactions: MessageReaction[];
  status: MessageStatus;
  error: string;
  /** True only on a message event that arrived in real time (drives the arrival animation). */
  live: boolean;
}

export interface CoreSettings {
  demoNetwork: boolean;
  sendReadReceipts: Record<string, boolean>;
  sendTyping: Record<string, boolean>;
  telegram: { apiId: number; apiHash: string };
}

export interface ChatFilter {
  network: string;
  archived: boolean;
  unreadOnly: boolean;
  query: string;
  directOnly?: boolean;
  serversOnly?: boolean;
}

export interface ChatFlags {
  pinned?: boolean | null;
  archived?: boolean | null;
  mutedUntil?: number | null;
}

/** Event kinds pushed by the core (CONTRACT section A). */
export type CoreEvent =
  | { kind: 'chat'; payload: Chat }
  | { kind: 'chat_removed'; payload: { roomId: string } }
  | { kind: 'message'; payload: Message }
  | { kind: 'message_removed'; payload: { roomId: string; eventId: string } }
  | { kind: 'typing'; payload: { roomId: string; names: string[] } }
  | { kind: 'account'; payload: Account }
  | { kind: 'core_error'; payload: { message: string } };

export type CoreEventKind = CoreEvent['kind'];

/* ---- Section C: app commands ---- */

export type Platform = 'macos' | 'windows' | 'linux' | 'android' | 'ios';
export type FormFactor = 'phone' | 'tablet' | 'desktop';

export interface AppInfo {
  version: string;
  platform: Platform;
  formFactor: FormFactor;
}

export type HapticKind = 'selection' | 'light' | 'medium' | 'heavy' | 'success' | 'warning' | 'error';

export interface UnlockResult {
  unlocked: boolean;
}

export interface SendResult {
  eventId: string;
}

export interface MediaFile {
  path: string;
}

export interface HasMore {
  hasMore: boolean;
}

/** App preferences owned by Rust (persisted in prefs.json; notification and lock decisions live there). */
export interface AppPrefs {
  hideNotificationContent: boolean;
  notifications: boolean;
  appLock: boolean;
  lockAfterSecs: number;
  secureScreen: boolean;
  closeToTray: boolean;
  launchAtLogin: boolean;
  haptics: boolean;
  /** Network ids whose messages never raise a notification. */
  mutedNetworks: string[];
}

export type PickKind = 'media' | 'file';

/** A file chosen in the native dialog. Only these paths are accepted by sendMedia. */
export interface PickedFile {
  path: string;
  name: string;
  mime: string;
  size: number;
}

export interface AppLockState {
  locked: boolean;
}

export interface NotificationReply {
  roomId: string;
  text: string;
}

/** Media kinds accepted by sendMedia. */
export type MediaKind = 'image' | 'video' | 'audio' | 'file' | 'sticker' | 'voice';
