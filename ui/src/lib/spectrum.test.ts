import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import {
  bannerCss,
  blendForNetworks,
  contrastRatio,
  networkColor,
  networkCss,
  NETWORK_SLOTS,
  personHue,
  parseHex,
  SPECTRUM_DARK,
  SPECTRUM_LIGHT,
  sortBySpectrum,
  type ThemeName,
} from './spectrum';

/*
 * The tests read styles/tokens.css itself, so they check the values the browser will use, not a
 * copy. Each theme is resolved the same way the CSS cascade does: rules in source order whose
 * selector matches the theme, then var() references substituted.
 */

const css = readFileSync(fileURLToPath(new URL('../styles/tokens.css', import.meta.url)), 'utf8');

interface Rule {
  selector: string;
  decls: Map<string, string>;
}

function parseRules(source: string): Rule[] {
  const stripped = source.replace(/\/\*[\s\S]*?\*\//g, '');
  const rules: Rule[] = [];
  for (const match of stripped.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const decls = new Map<string, string>();
    for (const decl of (match[2] ?? '').split(';')) {
      const colon = decl.indexOf(':');
      if (colon < 0) continue;
      const name = decl.slice(0, colon).trim();
      if (name.startsWith('--')) decls.set(name, decl.slice(colon + 1).trim());
    }
    rules.push({ selector: (match[1] ?? '').trim(), decls });
  }
  return rules;
}

const SELECTORS: Record<ThemeName, string[]> = {
  light: [':root', ':root[data-theme="light"]'],
  dark: [':root', ':root:not([data-theme="light"])', ':root[data-theme="dark"]'],
  amoled: [':root', ':root:not([data-theme="light"])', ':root[data-theme="amoled"]'],
};

const RULES = parseRules(css);

function resolveTheme(theme: ThemeName): Map<string, string> {
  const raw = new Map<string, string>();
  for (const rule of RULES) {
    if (!SELECTORS[theme].includes(rule.selector)) continue;
    for (const [name, value] of rule.decls) raw.set(name, value);
  }
  const value = (name: string, depth = 0): string => {
    const v = raw.get(name);
    if (v === undefined) throw new Error(`token ${name} is not defined`);
    if (depth > 8) throw new Error(`token ${name} is a cycle`);
    return v.replace(/var\((--[\w-]+)\)/g, (_, dep: string) => value(dep, depth + 1));
  };
  return new Map([...raw.keys()].map((name) => [name, value(name)]));
}

const THEMES: ThemeName[] = ['dark', 'light', 'amoled'];
const RESOLVED = Object.fromEntries(THEMES.map((t) => [t, resolveTheme(t)])) as Record<ThemeName, Map<string, string>>;

function token(theme: ThemeName, name: string): string {
  const v = RESOLVED[theme].get(name);
  if (v === undefined) throw new Error(`missing ${name} for ${theme}`);
  return v;
}

describe('tokens.css', () => {
  it('defines the same token names in every theme', () => {
    const light = [...RESOLVED.light.keys()].sort();
    expect([...RESOLVED.dark.keys()].sort()).toEqual(light);
    expect([...RESOLVED.amoled.keys()].sort()).toEqual(light);
  });

  it('spectrum tokens match the TypeScript mirror', () => {
    SPECTRUM_DARK.forEach((hex, i) => expect(token('dark', `--spectrum-${i}`)).toBe(hex));
    SPECTRUM_DARK.forEach((hex, i) => expect(token('amoled', `--spectrum-${i}`)).toBe(hex));
    SPECTRUM_LIGHT.forEach((hex, i) => expect(token('light', `--spectrum-${i}`)).toBe(hex));
  });

  it('every network token points at its fixed slot', () => {
    for (const [network, slot] of Object.entries(NETWORK_SLOTS)) {
      expect(token('dark', `--net-${network}`)).toBe(SPECTRUM_DARK[slot]);
      expect(token('light', `--net-${network}`)).toBe(SPECTRUM_LIGHT[slot]);
    }
  });
});

/** Banner fills (DESIGN 3.3 v2): solid, bright, theme-independent. */
const BANNER_TOKENS = [
  ...SPECTRUM_DARK.map((_, i) => `--banner-spectrum-${i}`),
  '--banner-neutral',
  '--banner-warn',
  '--banner-danger',
];

describe('banner colours', () => {
  it('banner tokens are the same bright values in every theme', () => {
    SPECTRUM_DARK.forEach((hex, i) => {
      for (const theme of THEMES) expect(token(theme, `--banner-spectrum-${i}`), `${theme} slot ${i}`).toBe(hex);
    });
  });

  it('every network banner points at its bright slot', () => {
    for (const [network, slot] of Object.entries(NETWORK_SLOTS)) {
      expect(token('light', `--banner-${network}`)).toBe(SPECTRUM_DARK[slot]);
      expect(bannerCss(network)).toBe(`var(--banner-${network})`);
    }
  });

  it('--on-banner ink reaches 7:1 on every bright banner colour, in every theme', () => {
    for (const theme of THEMES) {
      const ink = token(theme, '--on-banner');
      for (const name of BANNER_TOKENS) {
        expect(contrastRatio(ink, token(theme, name)), `${theme} ${name}`).toBeGreaterThanOrEqual(7);
      }
    }
  });

  it('the light column (small marks on light pages) stays readable on the light surfaces', () => {
    for (const network of Object.keys(NETWORK_SLOTS)) {
      const mark = token('light', `--net-${network}`);
      for (const surface of ['--bg', '--surface']) {
        expect(contrastRatio(mark, token('light', surface)), `light ${network} on ${surface}`).toBeGreaterThanOrEqual(3);
      }
    }
  });

  it('every network colour is at least 3:1 against the page background, in every theme', () => {
    for (const theme of THEMES) {
      const bg = token(theme, '--bg');
      for (const network of Object.keys(NETWORK_SLOTS)) {
        const ratio = contrastRatio(token(theme, `--net-${network}`), bg);
        expect(ratio, `${theme} ${network}`).toBeGreaterThanOrEqual(3);
      }
    }
  });

  it('body text and muted text meet 4.5:1 on the background and surfaces, in every theme', () => {
    for (const theme of THEMES) {
      for (const surface of ['--bg', '--surface-high']) {
        const back = token(theme, surface);
        expect(contrastRatio(token(theme, '--on'), back), `${theme} on ${surface}`).toBeGreaterThanOrEqual(4.5);
        expect(contrastRatio(token(theme, '--on-muted'), back), `${theme} muted on ${surface}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  });
});

describe('spectrum helpers', () => {
  it('sorts networks in spectrum order, then by label', () => {
    const sorted = sortBySpectrum(['messenger', 'discord', 'whatsapp', 'instagram', 'telegram'], (n) => n);
    expect(sorted).toEqual(['instagram', 'whatsapp', 'telegram', 'discord', 'messenger']);
  });

  it('a single network blends to its own colour', () => {
    expect(blendForNetworks(['whatsapp'])).toBe(networkColor('whatsapp'));
  });

  it('blends along the shortest arc: messenger (pink) to instagram (red) across the wrap', () => {
    const gradient = blendForNetworks(['instagram', 'messenger']);
    expect(gradient.startsWith(`linear-gradient(90deg, ${networkColor('messenger')} 0.0%`)).toBe(true);
    expect(gradient.endsWith(`${networkColor('instagram')} 100.0%)`)).toBe(true);
  });

  it('person hues are deterministic and in range', () => {
    expect(personHue('@alice:chirper.local')).toBe(personHue('@alice:chirper.local'));
    const hue = personHue('@bob:chirper.local');
    expect(hue).toBeGreaterThanOrEqual(0);
    expect(hue).toBeLessThan(360);
  });

  it('unknown networks get an OKLCH colour and no --net token', () => {
    expect(networkCss('someday')).toMatch(/^oklch\(/);
    expect(networkCss('whatsapp')).toBe('var(--net-whatsapp)');
  });

  it('parses 3, 6 and 8 digit hex', () => {
    expect(parseHex('#fff')).toEqual({ r: 255, g: 255, b: 255, a: 1 });
    expect(parseHex('#00000080').a).toBeCloseTo(128 / 255, 5);
    expect(contrastRatio('#000000', '#FFFFFF')).toBeCloseTo(21, 5);
  });
});
