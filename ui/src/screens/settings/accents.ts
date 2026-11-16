import type { AccentId } from '../../lib/spectrum';

/**
 * The four accent stops of each preset, as in styles/tokens.css (`:root[data-accent=...]`). The
 * test in accents.test.ts reads the CSS and fails if these drift apart.
 */
export const ACCENT_STOPS: Readonly<Record<AccentId, readonly [string, string, string, string]>> = {
  aurora: ['#6D5EF8', '#8B5CF6', '#4F8DF7', '#35C7C0'],
  ember: ['#FF7A59', '#F65C7A', '#B94BA8', '#B94BA8'],
  tide: ['#0E7C86', '#1D9BB5', '#2FA8B8', '#2FA8B8'],
  nebula: ['#B44BEA', '#7B5CF6', '#2A1B4A', '#2A1B4A'],
  citrus: ['#F5B93B', '#F07A3C', '#D9523C', '#D9523C'],
  coral: ['#FF8A7A', '#FF5F8A', '#C7467E', '#C7467E'],
  moss: ['#3F8F6B', '#2E7D6B', '#1F5F58', '#1F5F58'],
  dusk: ['#7A5AD6', '#C05A9E', '#5A3B7A', '#5A3B7A'],
  ice: ['#7FD3E8', '#57A8F0', '#6C7BF5', '#6C7BF5'],
  sakura: ['#F7A8C4', '#E87AA8', '#B85C8A', '#B85C8A'],
  graphite: ['#5A6A6D', '#46545A', '#2E3A3D', '#2E3A3D'],
  mono: ['#E9EFF0', '#C3CDD0', '#9AA9AC', '#9AA9AC'],
};

/** A diagonal gradient through one preset's stops, for swatches. */
export function swatchGradient(id: AccentId): string {
  const [a, b, c, d] = ACCENT_STOPS[id];
  return `linear-gradient(135deg, ${a} 0%, ${b} 40%, ${c} 72%, ${d} 100%)`;
}
