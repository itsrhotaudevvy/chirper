/**
 * The Chirper rainbow (DESIGN.md 3.2, 3.3): each network owns one of 14 spectrum slots, colour-led lists
 * sort in slot order, combined contexts blend their members along the shortest arc, and people get
 * a deterministic hue. Hex values here mirror the --spectrum-N tokens in styles/tokens.css, which the
 * tests verify.
 */

export type ThemeName = 'dark' | 'light' | 'amoled';

/** Dark values; AMOLED uses these as well. */
export const SPECTRUM_DARK: readonly string[] = [
  '#FD7277', '#F87C3C', '#DE9216', '#BEA412', '#91B519', '#46C165', '#0DBF9F',
  '#14BBC2', '#11B5E4', '#56A9FF', '#8F9AFE', '#BA87FB', '#DD7AD9', '#F372AB',
];

/** Light values. */
export const SPECTRUM_LIGHT: readonly string[] = [
  '#C43E49', '#BB4F08', '#9F6601', '#887402', '#668007', '#0F8C3B', '#068870',
  '#01858A', '#0980A3', '#0D76CE', '#5F65D5', '#8955C5', '#A748A5', '#BB3E7B',
];

/** Fixed slot per network (DESIGN 3.2). `demo` is the slot used by the demo world. */
export const NETWORK_SLOTS: Readonly<Record<string, number>> = {
  instagram: 0,
  fluxer: 1,
  slack: 2,
  twitter: 3,
  gvoice: 4,
  whatsapp: 5,
  gmessages: 6,
  telegram: 7,
  bluesky: 8,
  linkedin: 9,
  signal: 10,
  discord: 11,
  demo: 12,
  messenger: 13,
};

export const SPECTRUM_SLOTS = SPECTRUM_DARK.length;

/** Settings categories take fixed spectrum slots (DESIGN 3.2). */
export const SETTINGS_SLOTS = {
  accounts: 7,
  notifications: 1,
  privacy: 10,
  appearance: 12,
  chats: 5,
} as const;

/** Accent presets (DESIGN 3.4). `id` is the data-accent attribute value. */
export const ACCENT_PRESETS = [
  { id: 'aurora', label: 'Aurora' },
  { id: 'ember', label: 'Ember' },
  { id: 'tide', label: 'Tide' },
  { id: 'nebula', label: 'Nebula' },
  { id: 'citrus', label: 'Citrus' },
  { id: 'coral', label: 'Coral' },
  { id: 'moss', label: 'Moss' },
  { id: 'dusk', label: 'Dusk' },
  { id: 'ice', label: 'Ice' },
  { id: 'sakura', label: 'Sakura' },
  { id: 'graphite', label: 'Graphite' },
  { id: 'mono', label: 'Mono' },
] as const;

export type AccentId = (typeof ACCENT_PRESETS)[number]['id'];

const HUE_START = 20;
const HUE_STEP = 360 / SPECTRUM_SLOTS;

/**
 * Slot for a network id. Unknown networks get the next slot after the fixed ones; their colour
 * is then an OKLCH value (see networkColor) because the static token set has only 14 entries.
 */
export function networkSlot(network: string): number {
  const known = NETWORK_SLOTS[network];
  if (known !== undefined) return known;
  return SPECTRUM_SLOTS + unknownIndex(network);
}

const unknownIds = new Map<string, number>();
function unknownIndex(network: string): number {
  let index = unknownIds.get(network);
  if (index === undefined) {
    index = unknownIds.size;
    unknownIds.set(network, index);
  }
  return index;
}

/** Colour of a network in a theme. Overflow slots fall back to an OKLCH colour. */
export function networkColor(network: string, theme: ThemeName = 'dark'): string {
  const slot = networkSlot(network);
  if (slot < SPECTRUM_SLOTS) {
    return (theme === 'light' ? SPECTRUM_LIGHT : SPECTRUM_DARK)[slot] ?? SPECTRUM_DARK[0] ?? '#FFFFFF';
  }
  const hue = (HUE_START + slot * HUE_STEP) % 360;
  return theme === 'light' ? `oklch(0.56 0.17 ${hue})` : `oklch(0.72 0.17 ${hue})`;
}

/** CSS colour for a network's page marks (rings, dots, pills): the --net token, theme-aware. */
export function networkCss(network: string): string {
  return network in NETWORK_SLOTS ? `var(--net-${network})` : networkColor(network);
}

/**
 * CSS colour for a network's banner: the bright column, the same in every theme (DESIGN 3.3 v2).
 * Known networks use their --banner-<id> token; overflow networks use the bright OKLCH colour.
 */
export function bannerCss(network: string): string {
  return network in NETWORK_SLOTS ? `var(--banner-${network})` : networkColor(network, 'dark');
}

/** Hue (0..360) of a slot in OKLCH terms, used to order and blend colours. */
export function slotHue(slot: number): number {
  return (HUE_START + slot * HUE_STEP) % 360;
}

/**
 * Sorts items in spectrum order (red to pink). Ties (same slot) are ordered by label.
 * Pass `networkOf` to read the network id of an item, and `labelOf` to break ties.
 */
export function sortBySpectrum<T>(
  items: readonly T[],
  networkOf: (item: T) => string,
  labelOf: (item: T) => string = networkOf,
): T[] {
  return [...items].sort((a, b) => {
    const bySlot = networkSlot(networkOf(a)) - networkSlot(networkOf(b));
    return bySlot !== 0 ? bySlot : labelOf(a).localeCompare(labelOf(b));
  });
}

/**
 * CSS gradient through the colours of several networks, along the shortest arc of the colour wheel
 * that contains them all. One network yields a solid colour.
 */
export function blendForNetworks(networks: readonly string[], theme: ThemeName = 'dark'): string {
  const unique = [...new Set(networks)];
  const first = unique[0];
  if (first === undefined) return networkColor('demo', theme);
  if (unique.length === 1) return networkColor(first, theme);

  const ordered = unique
    .map((id) => ({ id, angle: slotHue(networkSlot(id)) }))
    .sort((a, b) => a.angle - b.angle);
  const start = shortestArcStart(ordered.map((o) => o.angle));
  const arcs = ordered.map((o) => ({ id: o.id, offset: (o.angle - start + 360) % 360 }));
  arcs.sort((a, b) => a.offset - b.offset);
  const span = arcs[arcs.length - 1]?.offset ?? 0;
  const stops = arcs.map((a) => `${networkColor(a.id, theme)} ${span === 0 ? 0 : ((a.offset / span) * 100).toFixed(1)}%`);
  return `linear-gradient(90deg, ${stops.join(', ')})`;
}

/** Angle at which the minimal arc covering all the given angles starts (just after the largest gap). */
function shortestArcStart(angles: readonly number[]): number {
  let bestGap = -1;
  let start = angles[0] ?? 0;
  angles.forEach((angle, i) => {
    const next = angles[(i + 1) % angles.length] ?? 0;
    const gap = (next - angle + 360) % 360 || 360;
    if (gap > bestGap) {
      bestGap = gap;
      start = next;
    }
  });
  return start;
}

/** Deterministic hue (0..359) for a person id, used as the avatar background (FNV-1a). */
export function personHue(id: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < id.length; i++) {
    hash ^= id.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % 360;
}

/* ---- Colour maths (WCAG) ---- */

export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Parses #RGB, #RRGGBB or #RRGGBBAA. */
export function parseHex(hex: string): Rgba {
  let h = hex.trim().replace(/^#/, '');
  if (h.length === 3) h = h.split('').map((c) => c + c).join('');
  if (h.length === 6) h += 'FF';
  if (!/^[0-9a-fA-F]{8}$/.test(h)) throw new Error(`not a hex colour: ${hex}`);
  const n = (i: number): number => parseInt(h.slice(i, i + 2), 16);
  return { r: n(0), g: n(2), b: n(4), a: n(6) / 255 };
}

function channel(v: number): number {
  const c = v / 255;
  return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance of an opaque colour. */
export function luminance(hex: string): number {
  const { r, g, b } = parseHex(hex);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/** WCAG contrast ratio (1..21) between two opaque colours. */
export function contrastRatio(a: string, b: string): number {
  const la = luminance(a);
  const lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
