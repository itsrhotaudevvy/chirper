import { settings } from './stores/settings.svelte';

/** Durations (ms) used by scripted animations. They collapse to 80 ms when motion is reduced. */
export const DURATION = { quick: 120, base: 220, page: 280, spring: 440, bounce: 620, instant: 80 } as const;

const query = typeof window === 'undefined' ? undefined : window.matchMedia('(prefers-reduced-motion: reduce)');

/** True when motion should be reduced: the in-app setting, or the OS preference. */
export function reduceMotion(): boolean {
  return settings.reduceMotion || (query?.matches ?? false);
}

/** A duration that respects reduced motion. */
export function dur(ms: number): number {
  return reduceMotion() ? DURATION.instant : ms;
}

/** Keeps data-reduce-motion on the root in step with the setting (CSS reads it). */
export function syncReduceMotion(): void {
  settings.applyLocal();
}
