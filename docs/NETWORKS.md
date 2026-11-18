# Chirper Supported Networks & Protocols

Chirper brings 13 chat networks directly onto your device into a unified inbox with no intermediate servers. Below is the technical specification and support status for each network.

---

## 1. Network Feature Matrix

| Network | Protocol / Backend | Auth Method | E2EE Status | Media Support | Reactions & Replies | Read Receipts |
|---|---|---|---|---|---|---|
| **WhatsApp** | WhatsApp Web Protocol | QR Code pairing | Native E2EE (Noise + Signal) | Images, Video, Audio, Docs | Full | Full |
| **Signal** | Signal WebSocket / REST | QR Linked Device | Native E2EE (Signal Protocol) | Images, Video, Audio, Docs | Full | Full |
| **Telegram** | MTProto v2.0 | Phone + SMS / QR | Server-encrypted (Secret chats client-side) | Full multimedia & stickers | Full | Full |
| **Discord** | Discord Gateway / REST | Token (user or bot) | TLS | Images, Video, Embeds, Audio | Full + Custom Emojis | Full |
| **Slack** | Slack Events / Web API | OAuth / User Token | TLS | Files, Images, Snippets | Full + Custom Reactions | Channel markers |
| **Messenger** | Meta MQTT / Graph | Browser session / cookies | Native E2EE (where enabled) | Images, Audio, Video | Full | Full |
| **Instagram** | Meta MQTT / Private API | User / Pass / 2FA | TLS / E2EE DMs | Photos, Reels, Voice notes | Full | Full |
| **X (Twitter)**| Twitter DM API | OAuth 2.0 / Cookie | TLS | Images, Video, GIFs | Direct replies | Full |
| **Bluesky** | AT Protocol (`chat.bsky`) | Handle + App Password | TLS | Text, images | Basic | Basic |
| **Google Messages** | Google Messages Web (RCS) | QR Code web pairing | RCS E2EE supported | Rich RCS Media, Audio | Full | Full |
| **Google Voice** | Google Voice Web API | Google Account Session | TLS | SMS & MMS, Voicemail | Text replies | Basic |
| **LinkedIn** | LinkedIn Voyager API | Cookie session | TLS | Attachments, Images | Direct replies | Basic |
| **Fluxer** | Fluxer Decentralized API | Ed25519 Keypair | Native E2EE | Distributed storage | Not ported yet | Not ported yet |

---

## 2. Network Details & Authentication Flows

### 2.1 WhatsApp
- **Bridge**: `third_party/whatsapp` (mautrix-whatsapp / whatsmeow)
- **Pairing**: Generates a standard WhatsApp Multi-Device pairing QR code in the Chirper UI. Open WhatsApp on your phone -> Linked Devices -> Link a Device -> Scan QR.
- **Keys**: Session keys are stored in the local SQLCipher database and backed by the platform Keystore.

### 2.2 Signal
- **Bridge**: `third_party/signal` (mautrix-signal)
- **Pairing**: Connects as a secondary linked device (Desktop/Tablet mode) using libsignal. Scans the link QR code via Signal settings.
- **Sync**: Message history syncs upon successful link according to Signal protocol standards.

### 2.3 Telegram
- **Bridge**: `third_party/telegram` (mautrix-telegram)
- **Pairing**: Enter your phone number with country code. Chirper triggers Telegram's official login code via Telegram service notifications or SMS. Enter 2FA password if enabled.

### 2.4 Discord
- **Bridge**: `go/connectors/discord`
- **Pairing**: User authorization token, or a bot token starting with `Bot `. The Rust driver
  (`crates/chirper-net-discord`) owns the gateway connection and its heartbeats.

### 2.5 Google Messages (RCS)
- **Bridge**: `third_party/gmessages` (mautrix-gmessages)
- **Pairing**: Browser emulation of Messages for Web. QR code is scanned in Google Messages app -> Device pairing. Supports RCS read receipts and reactions.

### 2.6 Bluesky
- **Bridge**: `third_party/bluesky`
- **Pairing**: Enter your bsky handle (e.g. `user.bsky.social`) and an **App Password** created in Bluesky Settings -> App Passwords.

---

## 3. Visual Spectrum & Color Coding

In accordance with `docs/DESIGN.md`, each network is assigned a specific color across the visual spectrum:

- 🟣 **Telegram**: `#26A5E4` (Bright Sky Cyan)
- 🟢 **WhatsApp**: `#25D366` (Vibrant Emerald)
- 🔵 **Signal**: `#3A76F0` (Ultramarine Blue)
- 🟣 **Discord**: `#5865F2` (Blurple)
- 🟡 **Slack**: `#ECB22E` / `#E01E5A` (Amber Auburn)
- 🟠 **Instagram**: `#E1306C` (Fuchsia Magenta)
- 🔵 **Messenger**: `#0084FF` (Electric Royal)
- ⚫ **X (Twitter)**: `#71767B` (Graphite Neutral)
- 🦋 **Bluesky**: `#1185FE` (Azure Blue)
- 🟡 **Google Messages**: `#F4B400` / `#4285F4` (Google Canary)
- 🟢 **Google Voice**: `#0F9D58` (Forest Sage)
- 🔵 **LinkedIn**: `#0A66C2` (Cobalt Navy)
- 🔴 **Fluxer**: `#FF5500` (Coral Orange)

These color accents are applied to avatars, unread pills, timeline headers, and system trays to identify context at a glance.
