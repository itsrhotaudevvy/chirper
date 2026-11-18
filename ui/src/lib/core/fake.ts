/**
 * In-memory stand-in for the Go core, used in a plain browser (pnpm dev without Tauri). Ported
 * from FakeCore.kt: a demo universe, simulated login flows (user_input, cookies, display_and_wait,
 * complete), delivery receipts, echo replies, typing and older-history pages. Events are batched
 * every 16 ms like the real core-batch event.
 */
import { CoreError, type CoreApi } from './api';
import { createDemoWorld, DEMO_ME, DEMO_MY_NAME, messagePreview } from './demo-world';
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
  InputField,
  LoginState,
  LoginStep,
  LoginStepType,
  MediaFile,
  MediaKind,
  Message,
  MessageReaction,
  MessageReply,
  MessageStatus,
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

const TIMING = {
  waitStepMs: 2_500,
  sentMs: 650,
  deliveredMs: 1_600,
  replyTypingMs: 2_400,
  replyDelayMs: 2_200,
  olderDelayMs: 900,
  batchMs: 16,
} as const;

const OLDER_BATCH = 4;
const MAX_HISTORY_PAGES = 2;
const HOUR_MS = 3_600_000;
const DOMAIN = 'chirper.local';
const CANNED_REPLY_ROOMS = new Set(['!whatsapp.alice:chirper.local', '!signal.jonas:chirper.local']);
const CANNED_REPLIES = ['Sounds good to me', 'Let me check and get back to you', 'Haha yes 😄', 'Perfect, see you then'];

const MEDIA_KIND_TYPE: Record<MediaKind, string> = {
  image: 'image',
  video: 'video',
  audio: 'audio',
  file: 'file',
  sticker: 'sticker',
  voice: 'voice',
};

interface LoginProcess {
  network: string;
  steps: LoginStep[];
  index: number;
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));
const clone = <T>(value: T): T => structuredClone(value);

export class FakeCore implements CoreApi {
  private readonly networkList: Network[];
  private accountList: Account[];
  private readonly chats = new Map<string, Chat>();
  private readonly messages = new Map<string, Message[]>();
  private readonly historyPages = new Map<string, number>();
  private readonly logins = new Map<string, LoginProcess>();
  private readonly listeners = new Set<(events: CoreEvent[]) => void>();
  private pending: CoreEvent[] = [];
  private flushTimer: ReturnType<typeof setTimeout> | undefined;
  private loginCount = 0;
  private appPrefs: AppPrefs = {
    hideNotificationContent: false,
    notifications: true,
    appLock: false,
    lockAfterSecs: 60,
    secureScreen: false,
    closeToTray: true,
    launchAtLogin: false,
    haptics: true,
    mutedNetworks: [],
  };
  private settingsValue: CoreSettings = {
    demoNetwork: true,
    sendReadReceipts: { '*': true },
    sendTyping: { '*': true },
    telegram: { apiId: 0, apiHash: '' },
  };

  private readonly now: () => number;

  constructor(now: () => number = Date.now) {
    this.now = now;
    const world = createDemoWorld(now());
    this.networkList = world.networks;
    this.accountList = world.accounts;
    for (const chat of world.chats) this.chats.set(chat.roomId, chat);
    for (const [roomId, list] of world.messages) this.messages.set(roomId, list);
  }

  async start(): Promise<void> {
    return undefined;
  }

  async networks(): Promise<Network[]> {
    return clone(this.networkList);
  }

  async accounts(): Promise<Account[]> {
    return clone(this.accountList);
  }

  // ---- Login ----------------------------------------------------------------------------------

  async startLogin(args: { network: string; flowId: string }): Promise<LoginState> {
    this.loginCount++;
    const loginId = `demo-${args.network}-${this.loginCount}`;
    const processId = crypto.randomUUID();
    const process: LoginProcess = {
      network: args.network,
      steps: loginSteps(args.network, args.flowId, loginId),
      index: 0,
    };
    this.logins.set(processId, process);
    return loginState(processId, process);
  }

  async submitLoginInput(args: { processId: string; values: Record<string, string> }): Promise<LoginState> {
    return this.advance(args.processId, 'user_input');
  }

  async submitLoginCookies(args: { processId: string; cookies: Record<string, string> }): Promise<LoginState> {
    return this.advance(args.processId, 'cookies');
  }

  async waitLoginStep(args: { processId: string }): Promise<LoginState> {
    await sleep(TIMING.waitStepMs);
    return this.advance(args.processId, 'display_and_wait');
  }

  async cancelLogin(args: { processId: string }): Promise<void> {
    this.logins.delete(args.processId);
  }

  async logout(args: { network: string; loginId: string }): Promise<void> {
    this.accountList = this.accountList.filter((a) => !(a.network === args.network && a.loginId === args.loginId));
    this.emit({
      kind: 'account',
      payload: { network: args.network, loginId: args.loginId, name: '', avatarUrl: '', state: 'LOGGED_OUT', error: '', message: '' },
    });
  }

  private advance(processId: string, expected: LoginStepType): LoginState {
    const process = this.logins.get(processId);
    if (!process) throw new CoreError('unknown_process', 'unknown login process');
    const current = process.steps[process.index];
    if (current?.type !== expected) throw new CoreError('invalid_step', `login step is ${current?.type ?? 'none'}`);
    process.index = Math.min(process.index + 1, process.steps.length - 1);
    const step = process.steps[process.index];
    if (step?.type === 'complete' && step.complete) {
      const account: Account = {
        network: process.network,
        loginId: step.complete.loginId,
        name: `Demo ${process.network} account`,
        avatarUrl: '',
        state: 'CONNECTED',
        error: '',
        message: '',
      };
      this.accountList = [...this.accountList, account];
      this.emit({ kind: 'account', payload: account });
    }
    return loginState(processId, process);
  }

  // ---- Chats ----------------------------------------------------------------------------------

  async listChats(args: { filter: ChatFilter }): Promise<Chat[]> {
    const { filter } = args;
    const query = filter.query.toLowerCase();
    return [...this.chats.values()]
      .filter((c) => !filter.network || c.network === filter.network)
      .filter((c) => c.archived === filter.archived)
      .filter((c) => !filter.unreadOnly || c.unreadCount > 0)
      .filter((c) => !query || c.name.toLowerCase().includes(query))
      .sort((a, b) => b.lastActivity - a.lastActivity)
      .map((c) => clone(c));
  }

  async getChat(args: { roomId: string }): Promise<Chat | null> {
    const chat = this.chats.get(args.roomId);
    return chat ? clone(chat) : null;
  }

  async listMessages(args: { roomId: string; beforeOrder: number; limit: number }): Promise<Message[]> {
    const all = this.messages.get(args.roomId) ?? [];
    const eligible = args.beforeOrder === 0 ? all : all.filter((m) => m.order < args.beforeOrder);
    return clone(eligible.slice(-args.limit));
  }

  async syncHistory(args: { network: string }): Promise<void> {
    void args;
  }

  async loadOlder(args: { roomId: string }): Promise<HasMore> {
    const page = this.historyPages.get(args.roomId) ?? 0;
    if (page >= MAX_HISTORY_PAGES) return { hasMore: false };
    this.historyPages.set(args.roomId, page + 1);
    const existing = this.messages.get(args.roomId) ?? [];
    const oldest = existing.reduce((min, m) => Math.min(min, m.ts), this.now());
    const name = this.chats.get(args.roomId)?.name ?? '';
    const batch: Message[] = Array.from({ length: OLDER_BATCH }, (_, i) => {
      const ts = oldest - (i + 1) * HOUR_MS;
      return {
        ...blankMessage(args.roomId, `$history.${crypto.randomUUID()}`, ts),
        sender: `@history:${DOMAIN}`,
        senderName: name,
        body: `Older message ${page + 1}.${i + 1}`,
        status: 'read' as const,
      };
    });
    await sleep(TIMING.olderDelayMs);
    for (const m of batch) this.apply(m);
    return { hasMore: (this.historyPages.get(args.roomId) ?? 0) < MAX_HISTORY_PAGES };
  }

  async markUnread(args: { roomId: string; unread: boolean }): Promise<void> {
    this.updateChat(args.roomId, (c) => ({ ...c, unreadCount: args.unread ? Math.max(c.unreadCount, 1) : 0 }));
  }

  async markRead(args: { roomId: string; eventId: string }): Promise<void> {
    void args.eventId;
    this.updateChat(args.roomId, (c) => ({ ...c, unreadCount: 0, mentionCount: 0 }));
  }

  async setChatFlags(args: { roomId: string; flags: ChatFlags }): Promise<void> {
    const { flags } = args;
    this.updateChat(args.roomId, (c) => ({
      ...c,
      pinned: flags.pinned ?? c.pinned,
      archived: flags.archived ?? c.archived,
      mutedUntil: flags.mutedUntil ?? c.mutedUntil,
    }));
  }

  // ---- Messaging ------------------------------------------------------------------------------

  async sendText(args: { roomId: string; text: string; replyTo: string; txnId: string }): Promise<SendResult> {
    const message: Message = {
      ...this.outgoing(args.roomId, 'text', args.text),
      replyTo: this.findReply(args.roomId, args.replyTo),
    };
    this.apply(message);
    this.scheduleDelivery(args.roomId, message.eventId);
    return { eventId: message.eventId };
  }

  async sendMedia(args: {
    roomId: string;
    path: string;
    mime: string;
    fileName: string;
    caption: string;
    kind: MediaKind;
    replyTo: string;
    txnId: string;
  }): Promise<SendResult> {
    const message: Message = {
      ...this.outgoing(args.roomId, MEDIA_KIND_TYPE[args.kind], args.caption),
      media: {
        url: `mxc://${DOMAIN}/local-${crypto.randomUUID()}`,
        mime: args.mime,
        size: 0,
        width: 0,
        height: 0,
        durationMs: 0,
        fileName: args.fileName,
        thumbnailUrl: '',
        blurhash: '',
      },
      replyTo: this.findReply(args.roomId, args.replyTo),
    };
    this.apply(message);
    this.scheduleDelivery(args.roomId, message.eventId);
    return { eventId: message.eventId };
  }

  async edit(args: { roomId: string; eventId: string; text: string }): Promise<void> {
    const target = this.find(args.roomId, args.eventId);
    if (!target.fromMe) throw new CoreError('forbidden', 'only your own messages can be edited');
    this.apply({ ...target, body: args.text, edited: true });
  }

  async redact(args: { roomId: string; eventId: string }): Promise<void> {
    const target = this.find(args.roomId, args.eventId);
    this.apply({ ...target, redacted: true, body: '', media: null });
  }

  async react(args: { roomId: string; eventId: string; key: string }): Promise<void> {
    this.updateReactions(args.roomId, args.eventId, (list) => {
      const existing = list.find((r) => r.key === args.key);
      if (!existing) return [...list, { key: args.key, count: 1, fromMe: true, senders: [DEMO_MY_NAME] }];
      if (existing.fromMe) return list;
      return list.map((r) =>
        r.key === args.key ? { ...r, fromMe: true, senders: [...r.senders, DEMO_MY_NAME] } : r,
      );
    });
  }

  async unreact(args: { roomId: string; eventId: string; key: string }): Promise<void> {
    this.updateReactions(args.roomId, args.eventId, (list) =>
      list.flatMap((r): MessageReaction[] => {
        if (r.key !== args.key || !r.fromMe) return [r];
        const rest = r.senders.filter((s) => s !== DEMO_MY_NAME);
        return rest.length === 0 ? [] : [{ ...r, fromMe: false, senders: rest }];
      }),
    );
  }

  async setTyping(args: { roomId: string; typing: boolean }): Promise<void> {
    void args;
  }

  async mediaFile(args: { url: string }): Promise<MediaFile> {
    void args;
    throw new CoreError('unsupported', 'media files are not available in the demo core');
  }

  async search(args: { query: string; limit: number }): Promise<Message[]> {
    const q = args.query.toLowerCase();
    return [...this.messages.values()]
      .flat()
      .filter((m) => m.body.toLowerCase().includes(q))
      .sort((a, b) => b.order - a.order)
      .slice(0, args.limit)
      .map((m) => clone(m));
  }

  async settings(): Promise<CoreSettings> {
    return clone(this.settingsValue);
  }

  async setSettings(args: { settings: CoreSettings }): Promise<void> {
    this.settingsValue = clone(args.settings);
  }

  // ---- App commands ---------------------------------------------------------------------------

  async appInfo(): Promise<AppInfo> {
    const ua = globalThis.navigator?.userAgent ?? '';
    const platform: AppInfo['platform'] = /iPhone|iPad/.test(ua)
      ? 'ios'
      : /Android/.test(ua)
        ? 'android'
        : /Windows/.test(ua)
          ? 'windows'
          : /Linux/.test(ua)
            ? 'linux'
            : 'macos';
    const mobile = platform === 'ios' || platform === 'android';
    return { version: '0.1.0-demo', platform, formFactor: mobile ? 'phone' : 'desktop' };
  }

  async haptic(args: { kind: HapticKind }): Promise<void> {
    void args;
  }

  async setBadge(args: { count: number }): Promise<void> {
    void args;
  }

  async notifyMessage(args: { roomId: string; title: string; body: string; avatarPath: string }): Promise<void> {
    void args;
  }

  async lockStatus(): Promise<{ locked: boolean }> {
    return { locked: false };
  }

  async unlock(): Promise<UnlockResult> {
    return { unlocked: true };
  }

  async setSecureScreen(args: { enabled: boolean }): Promise<void> {
    void args;
  }

  async openExternal(args: { url: string }): Promise<void> {
    if (!/^https?:\/\//.test(args.url)) throw new CoreError('invalid_url', 'only http(s) links can be opened');
    globalThis.open?.(args.url, '_blank', 'noopener');
  }

  async openLoginWebview(args: {
    url: string;
    extractJs?: string;
    waitForPattern?: string | undefined;
    userAgent?: string | undefined;
    title?: string | undefined;
  }): Promise<LoginWebviewResult> {
    void args;
    return { token: 'mock-discord-token-demo', cancelled: false };
  }

  async getAppPrefs(): Promise<AppPrefs> {
    return clone(this.appPrefs);
  }

  async setAppPrefs(args: { prefs: AppPrefs }): Promise<void> {
    this.appPrefs = clone(args.prefs);
  }

  async pickFiles(args: { kind: PickKind }): Promise<PickedFile[]> {
    void args;
    throw new CoreError('unsupported', 'file dialogs are not available in the browser demo');
  }

  onEvents(cb: (events: CoreEvent[]) => void): () => void {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  }

  // The demo core never locks or receives deep links or notification replies.
  onAppLock(cb: (state: AppLockState) => void): () => void {
    void cb;
    return () => undefined;
  }

  onDeepLink(cb: (url: string) => void): () => void {
    void cb;
    return () => undefined;
  }

  onNotificationReply(cb: (reply: NotificationReply) => void): () => void {
    void cb;
    return () => undefined;
  }

  // ---- Internals ------------------------------------------------------------------------------

  private emit(event: CoreEvent): void {
    this.pending.push(event);
    this.flushTimer ??= setTimeout(() => this.flush(), TIMING.batchMs);
  }

  private flush(): void {
    this.flushTimer = undefined;
    const batch = this.pending;
    this.pending = [];
    for (const cb of this.listeners) cb(batch);
  }

  private find(roomId: string, eventId: string): Message {
    const m = this.messages.get(roomId)?.find((x) => x.eventId === eventId);
    if (!m) throw new CoreError('not_found', 'message not found');
    return m;
  }

  private findReply(roomId: string, eventId: string): MessageReply | null {
    if (!eventId) return null;
    const m = this.messages.get(roomId)?.find((x) => x.eventId === eventId);
    if (!m) return null;
    return { eventId: m.eventId, senderName: m.senderName, body: m.body || 'Attachment' };
  }

  private outgoing(roomId: string, type: string, body: string): Message {
    return {
      ...blankMessage(roomId, `$local.${crypto.randomUUID()}`, this.now()),
      sender: DEMO_ME,
      senderName: DEMO_MY_NAME,
      fromMe: true,
      type,
      body,
      status: 'sending',
    };
  }

  /** Simulates the network echo: sent, delivered, and for some chats a reply from the other side. */
  private scheduleDelivery(roomId: string, eventId: string): void {
    void (async () => {
      await sleep(TIMING.sentMs);
      this.setStatus(roomId, eventId, 'sent');
      await sleep(TIMING.deliveredMs);
      this.setStatus(roomId, eventId, 'delivered');
      if (!CANNED_REPLY_ROOMS.has(roomId)) return;
      const name = this.chats.get(roomId)?.name ?? 'Them';
      await sleep(TIMING.replyTypingMs);
      this.emit({ kind: 'typing', payload: { roomId, names: [name] } });
      await sleep(TIMING.replyDelayMs);
      this.emit({ kind: 'typing', payload: { roomId, names: [] } });
      const reply: Message = {
        ...blankMessage(roomId, `$${roomId.slice(1).split(/[.:]/)[1] ?? 'room'}.reply${crypto.randomUUID().slice(0, 8)}`, this.now()),
        sender: `@remote:${DOMAIN}`,
        senderName: name,
        body: CANNED_REPLIES[Math.floor(Math.random() * CANNED_REPLIES.length)] ?? CANNED_REPLIES[0] ?? '',
      };
      this.apply(reply);
    })();
  }

  private setStatus(roomId: string, eventId: string, status: MessageStatus): void {
    const m = this.messages.get(roomId)?.find((x) => x.eventId === eventId);
    if (m) this.apply({ ...m, status });
  }

  private updateReactions(
    roomId: string,
    eventId: string,
    transform: (list: MessageReaction[]) => MessageReaction[],
  ): void {
    const target = this.find(roomId, eventId);
    const reactions = transform(target.reactions).map((r) => ({ ...r, count: r.senders.length }));
    this.apply({ ...target, reactions });
  }

  private updateChat(roomId: string, fn: (c: Chat) => Chat): void {
    const current = this.chats.get(roomId);
    if (!current) throw new CoreError('not_found', 'chat not found');
    const next = fn(current);
    this.chats.set(roomId, next);
    this.emit({ kind: 'chat', payload: clone(next) });
  }

  /** Stores a message, refreshes its chat's preview, activity and unread count, and emits both. */
  private apply(message: Message): void {
    const list = this.messages.get(message.roomId) ?? [];
    const index = list.findIndex((m) => m.eventId === message.eventId);
    const next = index >= 0 ? list.map((m, i) => (i === index ? message : m)) : [...list, message];
    next.sort((a, b) => a.order - b.order);
    this.messages.set(message.roomId, next);

    const chat = this.chats.get(message.roomId);
    if (chat) {
      const isNewest = next.at(-1)?.eventId === message.eventId;
      const updated: Chat = isNewest
        ? {
            ...chat,
            lastActivity: message.ts,
            preview: {
              sender: message.fromMe ? DEMO_MY_NAME : message.senderName,
              body: messagePreview(message),
              type: message.type,
            },
          }
        : chat;
      if (index < 0 && !message.fromMe) updated.unreadCount = chat.unreadCount + 1;
      this.chats.set(message.roomId, updated);
      this.emit({ kind: 'chat', payload: clone(updated) });
    }
    this.emit({ kind: 'message', payload: clone(message) });
  }
}

function blankMessage(roomId: string, eventId: string, ts: number): Message {
  return {
    eventId,
    roomId,
    order: ts * 1000,
    sender: '',
    senderName: '',
    senderAvatar: '',
    fromMe: false,
    ts,
    type: 'text',
    body: '',
    formattedBody: '',
    media: null,
    replyTo: null,
    edited: false,
    redacted: false,
    reactions: [],
    status: 'sent',
    error: '',
    live: false,
  };
}

function loginState(processId: string, process: LoginProcess): LoginState {
  const step = process.steps[process.index];
  if (!step) throw new CoreError('internal', 'login has no steps');
  return { processId, step: clone(step) };
}

function loginSteps(network: string, flowId: string, loginId: string): LoginStep[] {
  const complete: LoginStep = {
    type: 'complete',
    stepId: 'complete',
    instructions: '',
    complete: { loginId },
  };
  const input = (stepId: string, instructions: string, fields: InputField[]): LoginStep => ({
    type: 'user_input',
    stepId,
    instructions,
    userInput: { fields },
  });
  const field = (id: string, name: string, type: string, description = ''): InputField => ({
    id,
    name,
    type,
    description,
    options: [],
    pattern: '',
  });
  switch (flowId) {
    case 'qr':
      return [
        {
          type: 'display_and_wait',
          stepId: 'qr-1',
          instructions: 'Scan this code with the app on your phone.',
          displayAndWait: { type: 'qr', data: `2@${crypto.randomUUID()}`, imageUrl: '' },
        },
        complete,
      ];
    case 'phone':
      return [
        input('phone-1', "We'll send a login code to your Telegram app.", [field('phone', 'Phone number', 'phone')]),
        input('phone-2', 'Enter the code you received.', [field('code', 'Login code', 'code')]),
        complete,
      ];
    case 'cookies':
      return [cookieStep(network), complete];
    case 'password':
      return [
        input('creds', 'Use the username and password for this account.', [
          field('username', 'Username', 'username'),
          field('password', 'Password', 'password'),
        ]),
        complete,
      ];
    case 'token':
      return [
        input('token', 'Paste the token. It is stored only on this device.', [field('token', 'Token', 'token')]),
        complete,
      ];
    case 'bot':
      return [
        input('bot', 'Create a bot in the developer portal and paste its token.', [
          field('token', 'Bot token', 'token'),
        ]),
        complete,
      ];
    default:
      throw new CoreError('unknown_flow', `unknown login flow ${network}/${flowId}`);
  }
}

function cookieStep(network: string): LoginStep {
  const [url, names] = cookieTarget(network);
  return {
    type: 'cookies',
    stepId: 'cookies',
    instructions: "Sign in below. We'll continue automatically once the session is ready.",
    cookies: {
      url,
      userAgent: '',
      extractJs: '',
      fields: names.map((name) => ({
        id: name,
        required: true,
        sources: [{ type: 'cookie', name, cookieDomain: new URL(url).hostname.replace(/^www\./, '') }],
      })),
    },
  };
}

function cookieTarget(network: string): [string, string[]] {
  switch (network) {
    case 'instagram':
      return ['https://www.instagram.com/accounts/login/', ['sessionid', 'ds_user_id']];
    case 'twitter':
      return ['https://x.com/i/flow/login', ['auth_token', 'ct0']];
    case 'linkedin':
      return ['https://www.linkedin.com/login', ['li_at']];
    case 'messenger':
      return ['https://www.messenger.com/login/', ['c_user', 'xs']];
    default:
      return ['https://voice.google.com/', ['SAPISID']];
  }
}
