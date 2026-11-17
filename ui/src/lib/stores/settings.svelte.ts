import { core, type AppPrefs, type CoreApi } from '../core';
import { ACCENT_PRESETS, type AccentId } from '../spectrum';

/** "system" follows the OS colour scheme; the other values force a theme (data-theme). */
export type ThemePref = 'system' | 'dark' | 'light' | 'amoled';

/** Preferences that only this webview needs. Kept in localStorage (a per-device convenience). */
export interface LocalPrefs {
  theme: ThemePref;
  accent: AccentId;
  reduceMotion: boolean;
  autoUpdate?: boolean;
  /** Blur behind the main desktop chat containers. On by default; can be turned off. */
  glass?: boolean;
}

const STORAGE_KEY = 'chirper.ui.v1';
const THEMES: readonly string[] = ['system', 'dark', 'light', 'amoled'];
const ACCENTS: readonly string[] = ACCENT_PRESETS.map((a) => a.id);

export const DEFAULT_LOCAL: LocalPrefs = { theme: 'system', accent: 'aurora', reduceMotion: false, autoUpdate: true, glass: true };

/** Until the core answers, these apply. The core's get_app_prefs replaces them at start-up. */
export const DEFAULT_APP_PREFS: AppPrefs = {
  hideNotificationContent: false,
  notifications: true,
  appLock: false,
  lockAfterSecs: 60,
  secureScreen: false,
  closeToTray: true,
  launchAtLogin: false,
  haptics: true,
  mutedNetworks: [],
};

/** Keeps only well-typed local values; anything else falls back to the default. */
export function sanitiseLocal(raw: unknown): LocalPrefs {
  const source = typeof raw === 'object' && raw !== null ? (raw as Record<string, unknown>) : {};
  return {
    theme: THEMES.includes(source.theme as string) ? (source.theme as ThemePref) : DEFAULT_LOCAL.theme,
    accent: ACCENTS.includes(source.accent as string) ? (source.accent as AccentId) : DEFAULT_LOCAL.accent,
    reduceMotion: typeof source.reduceMotion === 'boolean' ? source.reduceMotion : DEFAULT_LOCAL.reduceMotion,
    autoUpdate: typeof source.autoUpdate === 'boolean' ? source.autoUpdate : true,
    glass: typeof source.glass === 'boolean' ? source.glass : DEFAULT_LOCAL.glass ?? true,
  };
}

/**
 * Preferences. Local ones (theme, accent, reduce motion) are persisted in localStorage, wrapped in
 * try/catch because storage may be unavailable, and applied to the document as data-theme,
 * data-accent and data-reduce-motion. App prefs (notifications, app lock, secure screen, haptics,
 * ...) belong to Rust: they are read with getAppPrefs and written with setAppPrefs.
 */
class SettingsStore {
  theme = $state<ThemePref>(DEFAULT_LOCAL.theme);
  accent = $state<AccentId>(DEFAULT_LOCAL.accent);
  reduceMotion = $state(DEFAULT_LOCAL.reduceMotion);
  autoUpdate = $state(DEFAULT_LOCAL.autoUpdate ?? true);
  glass = $state(DEFAULT_LOCAL.glass ?? true);
  app = $state<AppPrefs>({ ...DEFAULT_APP_PREFS });

  constructor(private readonly api: CoreApi = core) {
    this.assignLocal(this.readLocal());
  }

  /** Loads the Rust-owned prefs. Call once at start-up; falls back to the defaults on error. */
  async loadApp(): Promise<void> {
    this.app = await this.api.getAppPrefs();
  }

  /** Changes local prefs, persists them and applies them to the document. */
  updateLocal(patch: Partial<LocalPrefs>): void {
    this.assignLocal({ ...this.localSnapshot(), ...patch });
    this.writeLocal();
    this.applyLocal();
  }

  /** Changes app prefs in the core. The local value is restored if the core rejects the change. */
  async updateApp(patch: Partial<AppPrefs>): Promise<void> {
    const previous = this.app;
    this.app = { ...previous, ...patch };
    try {
      await this.api.setAppPrefs({ prefs: this.app });
    } catch (err) {
      this.app = previous;
      throw err;
    }
  }

  /** Writes data-theme, data-accent and data-reduce-motion on the root element. */
  applyLocal(): void {
    if (typeof document === 'undefined') return;
    const root = document.documentElement;
    if (this.theme === 'system') root.removeAttribute('data-theme');
    else root.setAttribute('data-theme', this.theme);
    root.setAttribute('data-accent', this.accent);
    root.setAttribute('data-reduce-motion', String(this.reduceMotion));
    root.setAttribute('data-glass', String(this.glass));
  }

  private localSnapshot(): LocalPrefs {
    return { theme: this.theme, accent: this.accent, reduceMotion: this.reduceMotion, autoUpdate: this.autoUpdate, glass: this.glass };
  }

  private assignLocal(prefs: LocalPrefs): void {
    this.theme = prefs.theme;
    this.accent = prefs.accent;
    this.reduceMotion = prefs.reduceMotion;
    this.autoUpdate = prefs.autoUpdate ?? true;
    this.glass = prefs.glass ?? true;
  }

  private readLocal(): LocalPrefs {
    try {
      const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
      return sanitiseLocal(raw ? JSON.parse(raw) : {});
    } catch {
      return DEFAULT_LOCAL;
    }
  }

  private writeLocal(): void {
    try {
      globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(this.localSnapshot()));
    } catch {
      // Storage is optional (private windows, blocked site data); the choice still applies.
    }
  }
}

export const settings = new SettingsStore();
