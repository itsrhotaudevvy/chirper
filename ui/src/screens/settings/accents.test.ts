import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { ACCENT_PRESETS } from '../../lib/spectrum';
import { ACCENT_STOPS } from './accents';

const css = readFileSync(fileURLToPath(new URL('../../styles/tokens.css', import.meta.url)), 'utf8');

describe('ACCENT_STOPS', () => {
  it('covers every preset', () => {
    expect(Object.keys(ACCENT_STOPS).sort()).toEqual(ACCENT_PRESETS.map((p) => p.id).sort());
  });

  it('matches the --accent-N values in tokens.css', () => {
    for (const preset of ACCENT_PRESETS) {
      const block = css.match(new RegExp(`:root\\[data-accent="${preset.id}"\\][^{]*\\{([^}]*)\\}`));
      expect(block, preset.id).not.toBeNull();
      const stops = [1, 2, 3, 4].map((n) => block?.[1]?.match(new RegExp(`--accent-${n}:\\s*(#[0-9A-Fa-f]{6})`))?.[1]);
      expect(stops, preset.id).toEqual(ACCENT_STOPS[preset.id]);
    }
  });
});
