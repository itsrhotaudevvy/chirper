import type { LoginFlow, Network } from '../../lib/core';
import { bannerCss, sortBySpectrum } from '../../lib/spectrum';

/** One colour tile of the network picker. */
export interface NetworkTile {
  id: string;
  name: string;
  /** CSS colour of the tile: the network's bright banner colour. */
  colour: string;
  available: boolean;
  reason: string;
  flows: readonly LoginFlow[];
}

/** Tiles in spectrum order (red to pink); networks in the same slot are ordered by name. */
export function networkTiles(networks: readonly Network[]): NetworkTile[] {
  return sortBySpectrum(networks, (n) => n.id, (n) => n.name).map((n) => ({
    id: n.id,
    name: n.name,
    colour: bannerCss(n.id),
    available: n.available,
    reason: n.reason,
    flows: n.loginFlows,
  }));
}
