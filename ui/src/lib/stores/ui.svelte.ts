/** Layout mode from the window width: phone < 600 px, tablet < 900 px, split view from 900 px. */
export type LayoutMode = 'phone' | 'tablet' | 'split';

export function layoutFor(width: number): LayoutMode {
  if (width < 600) return 'phone';
  if (width < 900) return 'tablet';
  return 'split';
}

/** Inbox pane width in split view (DESIGN 7: 360 px, resizable 300-480). */
export const INBOX_WIDTH = { min: 300, default: 360, max: 480 } as const;
const LAYOUT_KEY = 'chirper.layout.v1';

/** Keeps a width inside the resize range; anything that is not a number falls back to the default. */
export function clampInboxWidth(value: unknown): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) return INBOX_WIDTH.default;
  return Math.round(Math.min(INBOX_WIDTH.max, Math.max(INBOX_WIDTH.min, value)));
}

/** Reads the persisted inbox width. Storage may be unavailable (private windows), so it never throws. */
function readInboxWidth(): number {
  try {
    const raw = globalThis.localStorage?.getItem(LAYOUT_KEY);
    return clampInboxWidth(raw ? (JSON.parse(raw) as { inboxWidth?: unknown }).inboxWidth : undefined);
  } catch {
    return INBOX_WIDTH.default;
  }
}

function writeInboxWidth(width: number): void {
  try {
    globalThis.localStorage?.setItem(LAYOUT_KEY, JSON.stringify({ inboxWidth: width }));
  } catch {
    // The width still applies for this session.
  }
}

class UiStore {
  width = $state(typeof window === 'undefined' ? 1024 : window.innerWidth);
  selectedRoomId = $state<string | null>(null);
  inboxWidth = $state(readInboxWidth());
  readonly mode = $derived(layoutFor(this.width));

  /** Tracks the window width. Returns the disposer. */
  attach(win: Window = window): () => void {
    const onResize = (): void => {
      this.width = win.innerWidth;
    };
    onResize();
    win.addEventListener('resize', onResize);
    return () => win.removeEventListener('resize', onResize);
  }

  select(roomId: string | null): void {
    this.selectedRoomId = roomId;
  }

  /** Sets the inbox width (clamped). Persisted when `persist` is true, which is the end of a drag. */
  setInboxWidth(width: number, persist = false): void {
    this.inboxWidth = clampInboxWidth(width);
    if (persist) writeInboxWidth(this.inboxWidth);
  }
}

export const ui = new UiStore();
