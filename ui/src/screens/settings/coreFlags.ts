/** Per-network flags in CoreSettings, where "*" is the default for networks without their own value. */
export type FlagMap = Readonly<Record<string, boolean>>;

/** Effective value of a flag for one network. */
export function flagFor(map: FlagMap, network: string, fallback = true): boolean {
  return map[network] ?? map['*'] ?? fallback;
}

/** A copy of the map with one network's value set. The default ("*") is left as it is. */
export function withFlag(map: FlagMap, network: string, value: boolean): Record<string, boolean> {
  return { ...map, [network]: value };
}
