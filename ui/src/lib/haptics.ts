import type { HapticKind } from './core';
import { core } from './core';
import { settings } from './stores/settings.svelte';

/** Minimum gap between two haptics, so a burst of events feels like one tap. */
const DEBOUNCE_MS = 40;
let lastAt = Number.NEGATIVE_INFINITY;

/**
 * Plays a haptic through the core (a no-op where the platform has none). Debounced, and silent when
 * the haptics app pref is off. Failures are ignored: a haptic is never required for a flow.
 */
export function haptic(kind: HapticKind): void {
  if (!settings.app.haptics) return;
  const now = performance.now();
  if (now - lastAt < DEBOUNCE_MS) return;
  lastAt = now;
  core.haptic({ kind }).catch(() => undefined);
}
