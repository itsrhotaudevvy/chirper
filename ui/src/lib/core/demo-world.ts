/**
 * Static demo universe served by FakeCore (ported from DemoWorld.kt): 13 networks, 8 accounts,
 * 15 chats (one archived) across 8 networks, about 60 messages with replies, reactions, media
 * placeholders and edits.
 */
import type {
  Account,
  Capabilities,
  Chat,
  LoginFlow,
  Message,
  MessageMedia,
  MessageReaction,
  MessageReply,
  MessageStatus,
  Network,
} from './types';

export const DEMO_ME = '@me:chirper.local';
export const DEMO_MY_NAME = 'You';

export interface DemoWorld {
  networks: Network[];
  accounts: Account[];
  chats: Chat[];
  messages: Map<string, Message[]>;
}

const MINUTE_MS = 60_000;

export function createDemoWorld(now: number): DemoWorld {
  const timelines = [
    familyGroup(now),
    aliceDm(now),
    devTeam(now),
    miraTelegram(now),
    jonasSignal(now),
    hikeGroup(now),
    sarahDm(now),
    nomadDm(now),
    pharmacy(now),
    landlordArchived(now),
    designCrit(now),
    jordanDm(now),
    rustHelp(now),
    synthLab(now),
    miraBluesky(now),
  ];
  return {
    networks: networks(),
    accounts: accounts(),
    chats: timelines.map((t) => t.chat),
    messages: new Map(timelines.map((t) => [t.chat.roomId, t.messages])),
  };
}

function accounts(): Account[] {
  const acc = (
    network: string,
    loginId: string,
    name: string,
    state: Account['state'],
    message = '',
  ): Account => ({ network, loginId, name, avatarUrl: '', state, error: '', message });
  return [
    acc('whatsapp', '+15550100100', '+1 555 010 0100', 'CONNECTED'),
    acc('telegram', 'demo_user', '@demo_user', 'CONNECTED'),
    acc('signal', '+15550100101', '+1 555 010 0101', 'CONNECTING'),
    acc('instagram', 'demo.studio', '@demo.studio', 'BAD_CREDENTIALS', 'Session expired. Log in again to keep receiving messages.'),
    acc('gmessages', 'demo-phone', 'Demo Phone', 'CONNECTED'),
    acc('slack', 'T0DEMO', 'Demo Workspace', 'CONNECTED'),
    acc('discord', 'demo-bot', 'Demo Bot', 'CONNECTED'),
    acc('bluesky', 'demo.bsky.social', '@demo.bsky.social', 'TRANSIENT_DISCONNECT', 'Reconnecting...'),
  ];
}

function networks(): Network[] {
  const qr = (name: string, description: string): LoginFlow => ({ id: 'qr', name, description });
  const web = (description: string): LoginFlow => ({ id: 'cookies', name: 'Log in with web', description });
  const list: Array<[string, string, LoginFlow[], boolean]> = [
    ['whatsapp', 'WhatsApp', [qr('Link a device', 'Open WhatsApp > Linked devices > Link a device')], false],
    [
      'telegram',
      'Telegram',
      [
        { id: 'phone', name: 'Phone number', description: 'Log in with your phone number' },
        qr('QR code', 'Scan from Telegram on another device'),
      ],
      false,
    ],
    ['signal', 'Signal', [qr('Link as secondary device', 'Scan with Signal on your phone')], false],
    [
      'instagram',
      'Instagram',
      [
        web('Sign in on instagram.com'),
        { id: 'password', name: 'Username & password', description: 'Sign in with your credentials' },
      ],
      false,
    ],
    ['messenger', 'Messenger', [web('Sign in on messenger.com')], false],
    ['twitter', 'X', [web('Sign in on x.com')], false],
    ['linkedin', 'LinkedIn', [web('Sign in on linkedin.com')], false],
    ['gmessages', 'Google Messages', [qr('Pair with QR code', 'Scan on messages.google.com')], false],
    ['gvoice', 'Google Voice', [web('Sign in on voice.google.com')], true],
    ['slack', 'Slack', [{ id: 'token', name: 'Workspace token', description: 'Paste a user token for your workspace' }], false],
    [
      'discord',
      'Discord',
      [
        { id: 'token', name: 'User token', description: 'Use your Discord account token' },
        { id: 'bot', name: 'Bot token', description: 'Connect a bot account' },
      ],
      false,
    ],
    ['bluesky', 'Bluesky', [{ id: 'password', name: 'App password', description: 'Use a Bluesky app password' }], false],
    [
      'fluxer',
      'Fluxer',
      [{ id: 'token', name: 'Server token', description: 'Enter your server domain and token' }],
      true,
    ],
  ];
  const stickers = new Set(['whatsapp', 'telegram', 'signal', 'gmessages', 'discord', 'fluxer', 'slack']);
  const voice = new Set(['whatsapp', 'telegram', 'signal', 'discord', 'fluxer', 'gmessages', 'slack']);
  return list.map(([id, name, loginFlows, beta]) => ({
    id,
    name,
    color: '',
    beta,
    loginFlows,
    capabilities: capabilities(id, stickers, voice),
    available: true,
    reason: '',
  }));
}

function capabilities(id: string, stickers: Set<string>, voice: Set<string>): Capabilities {
  return {
    edits: id !== 'gvoice',
    reactions: true,
    replies: true,
    redactions: true,
    typing: id !== 'gvoice',
    readReceipts: true,
    stickers: stickers.has(id),
    voice: voice.has(id),
  };
}

interface ChatSpec {
  name: string;
  isDm?: boolean;
  isGroup?: boolean;
  memberCount?: number;
  pinned?: boolean;
  archived?: boolean;
  unread?: number;
  mentions?: number;
  muteMinutes?: number;
  typing?: string[];
}

interface MessageSpec {
  minutesAgo: number;
  sender: string;
  senderName: string;
  fromMe?: boolean;
  body?: string;
  type?: string;
  media?: MessageMedia | null;
  replyBody?: string;
  reactions?: MessageReaction[];
  edited?: boolean;
  status?: MessageStatus;
}

interface Timeline {
  chat: Chat;
  messages: Message[];
}

/** Builds one chat with its messages, keeping ordering and last-activity/preview consistent. */
class TimelineBuilder {
  private readonly items: Message[] = [];
  private seq = 0;
  private spec: ChatSpec = { name: '' };

  constructor(
    private readonly now: number,
    private readonly network: string,
    private readonly roomId: string,
  ) {}

  setChat(spec: ChatSpec): void {
    this.spec = spec;
  }

  add(m: MessageSpec): void {
    this.seq++;
    const ts = this.now - m.minutesAgo * MINUTE_MS;
    const roomPart = this.roomId.slice(1).split(/[.:]/)[1] ?? 'room';
    const eventId = `$${this.network}.${roomPart.slice(0, 8)}${this.seq.toString(16).padStart(6, '0')}`;
    const original = m.replyBody === undefined ? null : this.findByBody(m.replyBody);
    const replyTo: MessageReply | null =
      m.replyBody === undefined
        ? null
        : { eventId: original?.eventId ?? '', senderName: original?.senderName ?? '', body: m.replyBody };
    const fromMe = m.fromMe ?? false;
    this.items.push({
      eventId,
      roomId: this.roomId,
      order: ts * 1000 + this.seq,
      sender: m.sender,
      senderName: m.senderName,
      senderAvatar: '',
      fromMe,
      ts,
      type: m.type ?? 'text',
      body: m.body ?? '',
      formattedBody: '',
      media: m.media ?? null,
      replyTo,
      edited: m.edited ?? false,
      redacted: false,
      reactions: m.reactions ?? [],
      status: fromMe ? (m.status ?? 'sent') : 'sent',
      error: '',
      live: false,
    });
  }

  messages(): Message[] {
    return [...this.items].sort((a, b) => a.order - b.order);
  }

  build(): Timeline {
    const messages = this.messages();
    const last = messages.at(-1);
    const s = this.spec;
    const chat: Chat = {
      roomId: this.roomId,
      network: this.network,
      loginId: 'demo',
      name: s.name,
      avatarUrl: '',
      topic: '',
      isDm: s.isDm ?? false,
      isGroup: s.isGroup ?? false,
      isSpace: false,
      memberCount: s.memberCount ?? (s.isDm ? 2 : 0),
      lastActivity: last?.ts ?? 0,
      preview: last
        ? { sender: last.fromMe ? DEMO_MY_NAME : last.senderName, body: messagePreview(last), type: last.type }
        : null,
      unreadCount: s.unread ?? 0,
      mentionCount: s.mentions ?? 0,
      pinned: s.pinned ?? false,
      archived: s.archived ?? false,
      mutedUntil: s.muteMinutes ? this.now + s.muteMinutes * MINUTE_MS : 0,
      typing: s.typing ?? [],
    };
    return { chat, messages };
  }

  private findByBody(body: string): Message | undefined {
    return [...this.items].reverse().find((m) => m.body === body);
  }
}

/** The chat-list preview text of a message (a redacted message reads "Message deleted"). */
export function messagePreview(m: Message): string {
  if (m.redacted) return 'Message deleted';
  if (m.body) return m.body;
  switch (m.type) {
    case 'image':
      return 'Photo';
    case 'voice':
      return 'Voice message';
    case 'file':
      return m.media?.fileName || 'File';
    case 'sticker':
      return 'Sticker';
    default:
      return '';
  }
}

function reaction(key: string, senders: string[], fromMe = false): MessageReaction {
  const all = fromMe ? [...senders, DEMO_MY_NAME] : senders;
  return { key, count: all.length, fromMe, senders: all };
}

function photo(id: string): MessageMedia {
  const [w, h] = (id.split('-').at(-1) ?? '0x0').split('x').map(Number);
  return media({ url: `mxc://chirper.local/${id}`, mime: 'image/png', size: 240_000, width: w, height: h });
}

function sticker(id: string): MessageMedia {
  return media({ url: `mxc://chirper.local/${id}`, mime: 'image/png', size: 9_000, width: 512, height: 512 });
}

function voice(durationMs: number): MessageMedia {
  return media({
    url: `mxc://chirper.local/voice-${durationMs}`,
    mime: 'audio/wav',
    size: durationMs * 16,
    durationMs,
  });
}

function file(id: string, fileName: string, mime: string, size: number): MessageMedia {
  return media({ url: `mxc://chirper.local/${id}`, mime, size, fileName });
}

function media(partial: Partial<MessageMedia> & Pick<MessageMedia, 'url'>): MessageMedia {
  return {
    mime: '',
    size: 0,
    width: 0,
    height: 0,
    durationMs: 0,
    fileName: '',
    thumbnailUrl: '',
    blurhash: '',
    ...partial,
  };
}

const person = (slug: string): string => `@${slug}:chirper.local`;

function familyGroup(now: number): Timeline {
  const t = new TimelineBuilder(now, 'whatsapp', '!whatsapp.family:chirper.local');
  t.setChat({ name: 'Family', isGroup: true, memberCount: 6, pinned: true, unread: 2 });
  t.add({ minutesAgo: 180, sender: person('whatsapp_mom'), senderName: 'Mom', body: 'Dinner at ours on Sunday?' });
  t.add({
    minutesAgo: 170,
    sender: person('whatsapp_dad'),
    senderName: 'Dad',
    type: 'image',
    body: 'Sunset from the pier',
    media: photo('photo-28-1200x1600'),
    reactions: [reaction('❤️', ['Mom', 'Sis'])],
  });
  t.add({
    minutesAgo: 165,
    sender: DEMO_ME,
    senderName: DEMO_MY_NAME,
    fromMe: true,
    body: "Yes! I'll bring dessert",
    status: 'read',
    reactions: [reaction('👍', ['Mom', 'Dad'], true)],
  });
  t.add({ minutesAgo: 150, sender: person('whatsapp_sis'), senderName: 'Sis', body: 'Can you ask Grandma about the recipe?' });
  t.add({
    minutesAgo: 120,
    sender: person('whatsapp_mom'),
    senderName: 'Mom',
    body: 'Yes please 🙏',
    replyBody: "Yes! I'll bring dessert",
  });
  t.add({ minutesAgo: 40, sender: person('whatsapp_dad'), senderName: 'Dad', body: 'Ok 6pm then', edited: true });
  t.add({ minutesAgo: 12, sender: person('whatsapp_sis'), senderName: 'Sis', body: 'lol' });
  return t.build();
}

function aliceDm(now: number): Timeline {
  const t = new TimelineBuilder(now, 'whatsapp', '!whatsapp.alice:chirper.local');
  t.setChat({ name: 'Alice Park', isDm: true, unread: 2 });
  t.add({ minutesAgo: 300, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: 'Hey! Did you get the tickets?', status: 'read' });
  t.add({ minutesAgo: 290, sender: person('whatsapp_alice'), senderName: 'Alice Park', body: 'Not yet, the site is down 😩' });
  t.add({ minutesAgo: 60, sender: person('whatsapp_alice'), senderName: 'Alice Park', body: 'Are we still on for 3?' });
  t.add({ minutesAgo: 58, sender: person('whatsapp_alice'), senderName: 'Alice Park', body: 'Also: call?' });
  return t.build();
}

function devTeam(now: number): Timeline {
  const t = new TimelineBuilder(now, 'telegram', '!telegram.devteam:chirper.local');
  t.setChat({ name: 'Dev Team', isGroup: true, memberCount: 42, unread: 12, mentions: 2 });
  t.add({ minutesAgo: 90, sender: person('telegram_sam'), senderName: 'Sam', body: 'Build #2411 is green 🎉' });
  t.add({
    minutesAgo: 80,
    sender: person('telegram_priya'),
    senderName: 'Priya',
    type: 'image',
    body: "Here's the new onboarding flow",
    media: photo('photo-265-1080x720'),
  });
  t.add({ minutesAgo: 70, sender: person('telegram_sam'), senderName: 'Sam', body: '@you can you check the login QR size on small phones?' });
  t.add({
    minutesAgo: 65,
    sender: DEMO_ME,
    senderName: DEMO_MY_NAME,
    fromMe: true,
    body: 'On it. 240px should fit, bumping it now.',
    status: 'read',
    reactions: [reaction('👍', ['Sam', 'Priya'])],
  });
  t.add({ minutesAgo: 30, sender: person('telegram_ravi'), senderName: 'Ravi', body: 'Standup moved to 10:30' });
  t.add({ minutesAgo: 6, sender: person('telegram_priya'), senderName: 'Priya', body: 'Anyone read the Compose 1.13 release notes?' });
  return t.build();
}

function miraTelegram(now: number): Timeline {
  const t = new TimelineBuilder(now, 'telegram', '!telegram.mira:chirper.local');
  t.setChat({ name: 'Mira Chen', isDm: true, muteMinutes: 60 * 24 * 7 });
  t.add({ minutesAgo: 2000, sender: person('telegram_mira'), senderName: 'Mira Chen', body: 'Sent you the draft.' });
  t.add({ minutesAgo: 1400, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: 'Thanks, reading tonight', status: 'read' });
  return t.build();
}

function jonasSignal(now: number): Timeline {
  const t = new TimelineBuilder(now, 'signal', '!signal.jonas:chirper.local');
  t.setChat({ name: 'Jonas', isDm: true });
  t.add({ minutesAgo: 500, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, type: 'voice', media: voice(9_300), status: 'read' });
  t.add({ minutesAgo: 480, sender: person('signal_jonas'), senderName: 'Jonas', type: 'voice', media: voice(14_200) });
  t.add({
    minutesAgo: 470,
    sender: person('signal_jonas'),
    senderName: 'Jonas',
    type: 'file',
    body: 'route-notes.txt',
    media: file('file-route', 'route-notes.txt', 'text/plain', 2148),
  });
  t.add({ minutesAgo: 20, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: 'Got it, thanks 👌', status: 'sent' });
  return t.build();
}

function hikeGroup(now: number): Timeline {
  const t = new TimelineBuilder(now, 'signal', '!signal.hike:chirper.local');
  t.setChat({ name: 'Weekend hike', isGroup: true, memberCount: 5, unread: 4, muteMinutes: 8 * 60 });
  t.add({ minutesAgo: 200, sender: person('signal_ana'), senderName: 'Ana', body: 'Trail is open again' });
  t.add({ minutesAgo: 190, sender: person('signal_tomas'), senderName: 'Tomas', type: 'sticker', media: sticker('sticker-200') });
  t.add({ minutesAgo: 100, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: 'Forecast looks great for Saturday', status: 'read' });
  t.add({ minutesAgo: 35, sender: person('signal_ana'), senderName: 'Ana', body: 'Who is driving?' });
  t.add({ minutesAgo: 33, sender: person('signal_tomas'), senderName: 'Tomas', body: 'I can' });
  t.add({ minutesAgo: 5, sender: person('signal_ana'), senderName: 'Ana', body: 'Perfect, 7am at the station' });
  return t.build();
}

function sarahDm(now: number): Timeline {
  const t = new TimelineBuilder(now, 'instagram', '!instagram.sarah:chirper.local');
  t.setChat({ name: 'sarah.designs', isDm: true, unread: 1, typing: ['sarah.designs'] });
  t.add({ minutesAgo: 400, sender: person('instagram_sarah'), senderName: 'sarah.designs', body: "Your last post's typography is so clean" });
  t.add({
    minutesAgo: 380,
    sender: DEMO_ME,
    senderName: DEMO_MY_NAME,
    fromMe: true,
    body: 'Thank you! Inter with a little tightening on the big titles.',
    status: 'read',
  });
  t.add({ minutesAgo: 20, sender: person('instagram_sarah'), senderName: 'sarah.designs', body: 'Would you share the palette?' });
  return t.build();
}

function nomadDm(now: number): Timeline {
  const t = new TimelineBuilder(now, 'instagram', '!instagram.nomad:chirper.local');
  t.setChat({ name: 'studio.nomad', isDm: true });
  t.add({ minutesAgo: 2880, sender: person('instagram_nomad'), senderName: 'studio.nomad', body: 'Shot the new series, sending soon' });
  t.add({ minutesAgo: 1000, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: "Can't wait", status: 'read' });
  t.add({ minutesAgo: 180, sender: person('instagram_nomad'), senderName: 'studio.nomad', body: "We're in the edit room now" });
  return t.build();
}

function pharmacy(now: number): Timeline {
  const t = new TimelineBuilder(now, 'gmessages', '!gmessages.pharmacy:chirper.local');
  t.setChat({ name: 'Riverside Pharmacy', isDm: true, unread: 1 });
  t.add({ minutesAgo: 1440, sender: person('gmessages_pharmacy'), senderName: 'Riverside Pharmacy', body: 'Your prescription is ready for pickup.' });
  return t.build();
}

function landlordArchived(now: number): Timeline {
  const t = new TimelineBuilder(now, 'gmessages', '!gmessages.landlord:chirper.local');
  t.setChat({ name: 'Old landlord', isDm: true, archived: true });
  t.add({ minutesAgo: 7200, sender: person('gmessages_landlord'), senderName: 'Old landlord', body: 'Deposit returned, thanks for the clean handover.' });
  return t.build();
}

function designCrit(now: number): Timeline {
  const t = new TimelineBuilder(now, 'slack', '!slack.design-crit:chirper.local');
  t.setChat({ name: '#design-crit', isGroup: true, memberCount: 9, unread: 5, mentions: 1 });
  t.add({ minutesAgo: 60, sender: person('slack_lena'), senderName: 'Lena', body: 'Pushed the new composer, feedback welcome' });
  t.add({ minutesAgo: 55, sender: person('slack_marco'), senderName: 'Marco', type: 'image', body: 'Composer states', media: photo('photo-150-1600x1000') });
  t.add({
    minutesAgo: 52,
    sender: DEMO_ME,
    senderName: DEMO_MY_NAME,
    fromMe: true,
    body: 'Love the send-button morph. Can we keep the mic longer?',
    status: 'read',
  });
  t.add({
    minutesAgo: 40,
    sender: person('slack_lena'),
    senderName: 'Lena',
    body: "Sure, I'll try 150ms.",
    replyBody: 'Love the send-button morph. Can we keep the mic longer?',
    reactions: [reaction('🙌', ['Marco'])],
  });
  t.add({ minutesAgo: 10, sender: person('slack_marco'), senderName: 'Marco', body: '@you can you check dark mode contrast on the bubbles?' });
  return t.build();
}

function jordanDm(now: number): Timeline {
  const t = new TimelineBuilder(now, 'slack', '!slack.jordan:chirper.local');
  t.setChat({ name: 'Jordan Lee', isDm: true });
  t.add({ minutesAgo: 1000, sender: person('slack_jordan'), senderName: 'Jordan Lee', body: 'Standup notes in the doc?' });
  t.add({ minutesAgo: 990, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: 'Added them', status: 'read' });
  return t.build();
}

function rustHelp(now: number): Timeline {
  const t = new TimelineBuilder(now, 'discord', '!discord.rust-help:chirper.local');
  t.setChat({ name: '#rust-help', isGroup: true, memberCount: 1200 });
  t.add({ minutesAgo: 300, sender: person('discord_kaito'), senderName: 'kaito', body: 'Anyone know why tokio select! drops futures?' });
  t.add({ minutesAgo: 280, sender: person('discord_ferris'), senderName: 'ferris_fan', body: 'Read the cancel safety section in the docs 👉' });
  t.add({
    minutesAgo: 270,
    sender: person('discord_kaito'),
    senderName: 'kaito',
    body: 'Thanks, that was it!',
    reactions: [reaction('👍', ['ferris_fan'])],
  });
  return t.build();
}

function synthLab(now: number): Timeline {
  const t = new TimelineBuilder(now, 'discord', '!discord.synth-lab:chirper.local');
  t.setChat({ name: 'Synth Lab', isGroup: true, memberCount: 84, unread: 3 });
  t.add({ minutesAgo: 15, sender: person('discord_release-bot'), senderName: 'release-bot', body: 'v0.9.2 is on TestFlight and the Play internal track' });
  t.add({ minutesAgo: 14, sender: person('discord_maya'), senderName: 'maya', body: '🔥 The filter tweaks sound great' });
  t.add({ minutesAgo: 8, sender: person('discord_jun'), senderName: 'jun', body: 'Merged the filter tweaks', edited: true });
  return t.build();
}

function miraBluesky(now: number): Timeline {
  const t = new TimelineBuilder(now, 'bluesky', '!bluesky.mira:chirper.local');
  t.setChat({ name: '@mira.bsky.social', isDm: true, unread: 1 });
  t.add({ minutesAgo: 4000, sender: person('bluesky_mira'), senderName: 'Mira', body: 'Saw your thread about on-device messaging!' });
  t.add({ minutesAgo: 3900, sender: DEMO_ME, senderName: DEMO_MY_NAME, fromMe: true, body: "Thanks, it's a long one 🧵", status: 'read' });
  t.add({ minutesAgo: 120, sender: person('bluesky_mira'), senderName: 'Mira', body: 'Would love to test the Android build' });
  return t.build();
}
