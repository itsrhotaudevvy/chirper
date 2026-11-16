/**
 * App navigation (docs/CONTRACT.md §D). The current route is mirrored to location.hash so a view
 * can be opened directly (#/chat/<roomId>, #/settings, #/add-network/<network>). Forward moves push
 * a history entry, so the system back gesture (Android, browser) reaches `back()` through popstate.
 * Phone and tablet moves cross-fade and slide with the View Transitions API (DESIGN 5), with a CSS
 * fallback. Screens never import each other; they call `router`.
 */
import { tick } from 'svelte';
import { ui } from './stores/ui.svelte';
import { reduceMotion } from './motion';

export type View = 'inbox' | 'chat' | 'settings' | 'add-network';

export interface Route {
  view: View;
  roomId?: string;
  network?: string;
}

/** Direction of a move, read by the CSS (`html[data-nav]`) to pick the slide direction. */
export type NavDirection = 'forward' | 'back';

type HistoryWrite = 'push' | 'replace' | 'none';

export function parseHash(hash: string): Route {
  const parts = hash.replace(/^#\/?/, '').split('/').filter(Boolean).map(safeDecode);
  switch (parts[0]) {
    case 'chat':
      return parts[1] ? { view: 'chat', roomId: parts[1] } : { view: 'inbox' };
    case 'settings':
      return { view: 'settings' };
    case 'add-network':
      return parts[1] ? { view: 'add-network', network: parts[1] } : { view: 'add-network' };
    default:
      return { view: 'inbox' };
  }
}

export function formatHash(route: Route): string {
  switch (route.view) {
    case 'chat':
      return route.roomId ? `#/chat/${encodeURIComponent(route.roomId)}` : '#/';
    case 'settings':
      return '#/settings';
    case 'add-network':
      return route.network ? `#/add-network/${encodeURIComponent(route.network)}` : '#/add-network';
    default:
      return '#/';
  }
}

/**
 * Route for a deep link such as `chirper://chat/<roomId>` (CONTRACT §C). Returns null for anything
 * the app does not handle, so an unknown link never changes the screen.
 */
export function parseDeepLink(url: string): Route | null {
  const match = /^chirper:\/\/chat\/([^?#]+)/i.exec(url.trim());
  const roomId = match?.[1] ? safeDecode(match[1]) : '';
  return roomId ? { view: 'chat', roomId } : null;
}

function safeDecode(part: string): string {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
}

/** True when the browser supports same-document view transitions and motion is not reduced. */
function canAnimate(): boolean {
  return typeof document !== 'undefined' && 'startViewTransition' in document && !reduceMotion();
}

class Router {
  route = $state<Route>({ view: 'inbox' });
  readonly view = $derived(this.route.view);
  /** Routes to return to, newest last. Each entry corresponds to one pushed history entry. */
  private history: Route[] = [];
  /** history.back() calls made by `back()` whose popstate must not pop the stack a second time. */
  private ownPops = 0;

  /** Starts following location.hash and the system back gesture. Returns the disposer. */
  attach(win: Window = window): () => void {
    const sync = (): void => this.set(parseHash(win.location.hash), 'none');
    sync();
    const onPop = (): void => {
      if (this.ownPops > 0) {
        this.ownPops -= 1;
        return;
      }
      const previous = this.history.pop();
      if (previous) this.move(previous, 'back', 'none');
    };
    win.addEventListener('hashchange', sync);
    win.addEventListener('popstate', onPop);
    return () => {
      win.removeEventListener('hashchange', sync);
      win.removeEventListener('popstate', onPop);
    };
  }

  /** Opens a view and remembers where we came from, so `back()` returns there. */
  go(view: View, params: Omit<Route, 'view'> = {}): void {
    const from = this.route;
    this.history.push(from);
    this.move({ view, ...params }, 'forward', 'push');
  }

  /** Replaces the current route (used for chat selection in split view, which has no stack). */
  replace(view: View, params: Omit<Route, 'view'> = {}): void {
    this.set({ view, ...params }, 'replace');
  }

  /** Opens a chat: pushed on phone and tablet, replaced in split view (where the list stays put). */
  openChat(roomId: string): void {
    if (ui.mode === 'split') this.replace('chat', { roomId });
    else this.go('chat', { roomId });
  }

  /** Returns to the previous route; the inbox when there is none. */
  back(): void {
    const previous = this.history.pop();
    if (!previous) {
      this.move({ view: 'inbox' }, 'back', 'replace');
      return;
    }
    this.move(previous, 'back', 'none');
    this.ownPops += 1;
    window.history.back();
  }

  /** Applies a route, animated for phone and tablet. `write` says how the URL follows the route. */
  private move(route: Route, direction: NavDirection, write: HistoryWrite): void {
    const commit = (): void => this.set(route, write);
    if (typeof document !== 'undefined') document.documentElement.dataset.nav = direction;
    if (ui.mode === 'split' || !canAnimate()) {
      commit();
      return;
    }
    void document.startViewTransition(() => {
      commit();
      return tick();
    });
  }

  /** Sets the route. `write`: 'push' adds a history entry, 'replace' rewrites the current one, 'none' leaves the URL alone. */
  private set(route: Route, write: HistoryWrite): void {
    this.route = route;
    ui.select(route.view === 'chat' ? (route.roomId ?? null) : ui.selectedRoomId);
    if (typeof window === 'undefined' || write === 'none') return;
    const hash = formatHash(route);
    if (write === 'push') window.history.pushState({ chirper: true }, '', hash);
    else if (window.location.hash !== hash) window.history.replaceState(null, '', hash);
  }
}

export const router = new Router();
