# Chirper — design spec (web/Tauri)

Implementation-ready. Every value is meant to be copied into `ui/src/styles/tokens.css` and the
components. Derived from Openchirp's `chirper-design.md`, rebuilt to be platform-independent:
no Material, no native widget look, one identical system on all five OSs.

## 0. The one rule: colour-coded banners, bold and few words

**Colour does the work that extra text and rows would otherwise do.** Instead of stacking labels,
badges and metadata side by side, each screen and each section opens with **one bold colour
banner**, and meaning is encoded in colour:

- **Platform = colour.** Every network has a brand colour (table §3.2). A chat's banner, a
  network's settings page, an account's status ring and the inbox section header all wear that
  colour. Users learn "green = WhatsApp, blurple = Discord" and never need to read a label.
- **State = colour.** Connected = the network colour at full strength; connecting = the same
  colour pulsing at 50 %; needs attention = `--warn` banner; error = `--danger` banner. One word
  on the banner ("Reconnect"), never a paragraph.
- **One banner per screen.** Full-bleed, starts at the very top edge (under the status bar /
  titlebar — on desktop the banner *is* the titlebar drag region), 1 line of bold type
  (`display`), fades into the page at 75 % of its height. Content never sits on colour.
- **Bold, little information.** A row shows at most: avatar, name, one line, one time, one count.
  Anything else (platform, member count, labels) is either colour or one tap away.
- **People keep their own colour** (deterministic hue from their id) inside avatars; the
  platform appears as a small colour ring/badge — the banner is the platform, the circle is
  the person.
- Combined (multi-network) contexts blend the member network colours into one smooth gradient.

## 1. Brand

Wordmark `chirper`, lowercase, Inter SemiBold, tracking −2 %. Mark: a speech bubble with a chirp
notch (108×108 viewBox path below), white on the Aurora gradient (135°).

```
Aurora: #6D5EF8 → #8B5CF6 → #4F8DF7 → #35C7C0
mark:   M40,28 H68 A14,14 0 0 1 82,42 V56 A14,14 0 0 1 68,70 H59 L46,80 V70 H40
        A14,14 0 0 1 26,56 V42 A14,14 0 0 1 40,28 Z
```

## 2. Type (Inter, self-hosted woff2 — never a font CDN)

| token | size/line/weight | use |
|---|---|---|
| `--t-display` | 30/34/700, −0.5 px | banner titles |
| `--t-title` | 22/26/650 | sheet titles |
| `--t-headline` | 17/22/600 | row primary, chat name in compact header |
| `--t-body` | 15.5/21/400 | messages, previews |
| `--t-label` | 12.5/16/550 | times, counts (tabular-nums) |

Max two weights per screen. Font scale follows the OS text size (rem-based, 87.5–200 %).

## 3. Colour

### 3.1 Surfaces (`[data-theme]` = dark | light | amoled; default follows the OS)

| token | dark | light | amoled |
|---|---|---|---|
| `--bg` | #0E1517 | #F6F8F9 | #000000 |
| `--surface` | #131B1D | #FFFFFF | #050505 |
| `--surface-high` | #1A2427 | #EEF2F3 | #0B0B0B |
| `--on` | #E9EFF0 | #10181A | #F2F6F7 |
| `--on-muted` | #9AA9AC | #5A6A6D | #9AA9AC |
| `--outline` | #253235 | #DCE3E5 | #1A1A1A |
| `--warn` | #F5B93B | #B7791F | #F5B93B |
| `--danger` | #F2555A | #C8323A | #F2555A |

### 3.2 The Chirper rainbow (`--net-<id>`, `--spectrum-<n>`)

Colour coding is organised as **one rainbow spectrum**. Brand colours cluster in blue (7 of 13
networks), so they are *not* used for coding. Instead the 14 contexts occupy 14 evenly spaced
OKLCH hues (start 20°, step 25.7°, L 0.72 / C 0.17 dark, L 0.56 / C 0.17 light, chroma reduced
only to stay in sRGB), each network on the slot nearest its brand identity. Every pair of
networks is therefore distinguishable at a glance, and the set reads as a rainbow.

| slot | network | hue | dark | light |
|---|---|---|---|---|
| 0 | instagram | 20 | #FD7277 | #C43E49 |
| 1 | fluxer | 46 | #F87C3C | #BB4F08 |
| 2 | slack | 71 | #DE9216 | #9F6601 |
| 3 | twitter (X) | 97 | #BEA412 | #887402 |
| 4 | gvoice | 123 | #91B519 | #668007 |
| 5 | whatsapp | 149 | #46C165 | #0F8C3B |
| 6 | gmessages | 174 | #0DBF9F | #068870 |
| 7 | telegram | 200 | #14BBC2 | #01858A |
| 8 | bluesky | 226 | #11B5E4 | #0980A3 |
| 9 | linkedin | 251 | #56A9FF | #0D76CE |
| 10 | signal | 277 | #8F9AFE | #5F65D5 |
| 11 | discord | 303 | #BA87FB | #8955C5 |
| 12 | demo | 329 | #DD7AD9 | #A748A5 |
| 13 | messenger | 354 | #F372AB | #BB3E7B |

AMOLED uses the dark values. Unknown future networks take the next free slot (the step halves
to 12.9° when all 14 are used). The Network JSON `color` field is ignored for coding.

**Spectrum order is the sort order for anything shown by colour:** the network picker grid,
account lists, the inbox network filter strip, section banners in "group by network" mode and
the network list in settings all run red → orange → yellow → green → cyan → blue → violet → pink.
Within a slot, order alphabetically. Chats inside the inbox stay sorted by time (pinned first);
only grouped/colour-led views use spectrum order.

**Settings categories** take spectrum colours too, so settings is itself a rainbow of banners:
Accounts = slot 7, Notifications = slot 1, Privacy & security = slot 10, Appearance = slot 12,
Chats & storage = slot 5, About = neutral `--on-muted`.

**Combined chats** (several networks) use a gradient through their slots in spectrum order —
the shortest arc around the wheel — so mixed contexts still read as a piece of the rainbow.

### 3.3 Banner recipe (v2 — bold, crisp)

Banners are **solid blocks of bright colour with a crisp edge** — no veil, no fade into the page,
no dark band. In **every theme** banners use the bright spectrum values (the *dark* column of §3.2)
with `--on-banner: #0B0F10` text: every slot measures ≥ 7:1, so no scrim or veil is needed and the
colour stays at full strength in light mode too. The light column is only for small marks on light
pages (rings, pills, coloured text).

- Single context: `background: C` plus one soft luminous highlight
  `radial-gradient(80% 120% at 85% 0%, color-mix(in oklch, C, white 22%), transparent 70%)`.
- Several networks: a smooth gradient through their slots in spectrum order (shortest arc).
- Shape: full-bleed, bottom corners radius 28 px on phones (24 on desktop), so it reads as a
  banner, not a page wash. Content below starts 16 px after the edge.
- Text sits directly on the colour. Never put text in a chip/box/scrim rectangle.
- Section banners and settings category cards follow the same recipe at smaller sizes.
- Filter/legend dots are **solid** colour discs (selected = 3 px `--on` ring around the disc).

### 3.4 Accent presets (user-selectable, 12)

Aurora, Ember, Tide, Nebula, Citrus, Coral, Moss, Dusk, Ice, Sakura, Graphite, Mono — stops as in
the Openchirp spec. The accent colours app chrome only (inbox banner, focus rings, send button);
network colour always wins inside a network context.

## 4. Components

- **Banner** (props: colour(s), title, optional single action, optional status word). 168 px on
  phones (plus safe-area-inset-top), 120 px on desktop where it doubles as the titlebar.
- **Inbox row** 68 px: avatar 48 with 2 px network ring, name `headline`, preview `body` 1 line,
  time `label`, unread pill 20 px in the network colour. No dividers. Hover (desktop) = `--surface-high`.
- **Section banner** in lists: a slim 32 px colour band with one bold word (network or "Pinned").
- **Messages**: no bubble for plain text — type on the page; outgoing text in the chat colour mixed
  35 % into `--on`, right-aligned. Media/quotes/reactions use containers with radius 18/6 grouping.
  Gap 3 px inside a sender group, 12 px between groups; time + ticks only on the group's last message.
  Jumbo emoji: 52 px for 1–2 emoji, 40 px for 3.
- **Composer**: capsule, translucent `--surface` 92 % + backdrop blur, hairline top; send/mic 44 px
  circle in the chat colour, morphs on text.
- **Sheet**: bottom sheet on phones, centred popover/side panel on desktop; one title, no more than
  6 rows per group.
- Touch targets ≥ 44 px; keyboard focus ring 2 px accent; every icon button has an aria-label.

## 5. Motion (`ui/src/lib/motion.ts`, CSS custom properties)

| token | value | use |
|---|---|---|
| `--ease-out` | cubic-bezier(.2,.8,.2,1) | most transitions |
| `--spring` | `linear()` spring approximation (damping .8, stiffness 380) | sheets, avatar flight |
| `--spring-bounce` | `linear()` (damping .55, stiffness 420) | reactions, send |
| `--dur-quick` 120 ms · `--dur-base` 220 ms · `--dur-page` 280 ms | | |

Signature animations: chat open uses the View Transitions API (avatar + banner morph from the row);
live message arrival rises 16 px + fades (only `live` messages); banner colour cross-fades when the
network changes; send button rotates 90° on send; reaction pop .7→1.1→1; typing = three dots with a
travelling gradient. Only `transform`/`opacity` animate. `prefers-reduced-motion`: durations ≤ 80 ms,
no infinite animations. Haptics pair with motion: selection on tab/segment change, light on send,
success on login complete, warning on errors, medium on long-press menus.

## 6. Friction rules (show only what is necessary, when it is necessary)

1. First launch: one banner, one sentence, one button ("Add a network"). No tour.
2. Network picker shows colour tiles only; flows appear only after a tile is tapped; a single flow
   is chosen automatically.
3. Settings show 5 rows by default; advanced options appear behind "More" inside the relevant
   network's own banner page.
4. Composer extras (attach, emoji) appear on focus; reply/edit state is a coloured strip, not a dialog.
5. Errors are a banner state with one action word, never a modal, except destructive confirmations.
6. Empty states: one line of text and one action.
7. Desktop: keyboard first — ⌘/Ctrl+K jump to chat, ↑ edit last, Esc close, ⌘/Ctrl+1..9 pinned chats.

## 7. Layout

- `< 600 px` phone: stack navigation (inbox → chat) with edge-swipe back.
- `600–899` tablet portrait: stack with wider gutters.
- `≥ 900` split view: inbox 360 px (resizable 300–480) + chat; settings open as a side panel.
- Safe areas via `env(safe-area-inset-*)`; macOS overlay titlebar leaves 78 px for traffic lights.
