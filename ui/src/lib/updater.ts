import { core, type Platform } from './core';

export interface GitHubReleaseAsset {
  name: string;
  browser_download_url: string;
  size: number;
  content_type: string;
}

export interface GitHubRelease {
  tag_name: string;
  name: string;
  body: string;
  published_at: string;
  html_url: string;
  assets: GitHubReleaseAsset[];
}

export type UpdaterStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'up-to-date' | 'error';

export interface UpdaterState {
  status: UpdaterStatus;
  currentVersion: string;
  latestVersion: string;
  releaseNotes: string;
  downloadUrl: string;
  assetName: string;
  error?: string;
  lastChecked?: number;
  dismissed?: boolean;
}

const GITHUB_REPO = 'itsrhotaudevvy/chirper';

class UpdaterStore {
  private _state: UpdaterState = {
    status: 'idle',
    currentVersion: '0.1.2',
    latestVersion: '',
    releaseNotes: '',
    downloadUrl: '',
    assetName: '',
  };
  private _subscribers = new Set<(state: UpdaterState) => void>();

  get state(): UpdaterState {
    return this._state;
  }

  subscribe(fn: (state: UpdaterState) => void): () => void {
    fn(this._state);
    this._subscribers.add(fn);
    return () => this._subscribers.delete(fn);
  }

  private notify(): void {
    for (const fn of this._subscribers) {
      fn(this._state);
    }
  }

  dismiss(): void {
    this._state = { ...this._state, dismissed: true };
    this.notify();
  }

  /**
   * Compares semver strings. Returns:
   *  1 if a > b
   * -1 if a < b
   *  0 if a == b
   */
  compareVersions(a: string, b: string): number {
    const clean = (v: string) =>
      v
        .replace(/^v/, '')
        .split('-')[0]
        ?.split('.')
        .map((n) => parseInt(n, 10) || 0) ?? [0, 0, 0];
    const pa = clean(a);
    const pb = clean(b);
    const len = Math.max(pa.length, pb.length);
    for (let i = 0; i < len; i++) {
      const na = pa[i] ?? 0;
      const nb = pb[i] ?? 0;
      if (na > nb) return 1;
      if (na < nb) return -1;
    }
    return 0;
  }

  /** Picks the best asset matching the client platform and architecture. */
  pickAsset(assets: GitHubReleaseAsset[], platform: Platform): GitHubReleaseAsset | undefined {
    if (!assets || assets.length === 0) return undefined;

    switch (platform) {
      case 'android':
        return (
          assets.find((a) => a.name.endsWith('.apk') && a.name.includes('universal')) ??
          assets.find((a) => a.name.endsWith('.apk'))
        );
      case 'macos':
        return (
          assets.find((a) => a.name.endsWith('.dmg')) ??
          assets.find((a) => a.name.endsWith('.zip') && a.name.includes('macos'))
        );
      case 'windows':
        return (
          assets.find((a) => a.name.endsWith('.exe')) ??
          assets.find((a) => a.name.endsWith('.msi'))
        );
      case 'linux':
        return (
          assets.find((a) => a.name.endsWith('.AppImage')) ??
          assets.find((a) => a.name.endsWith('.deb'))
        );
      case 'ios':
        return assets.find((a) => a.name.endsWith('.ipa'));
      default:
        return assets[0];
    }
  }

  /** Initiates download of the update asset. */
  async download(url: string, assetName: string): Promise<void> {
    this._state = { ...this._state, status: 'downloading', downloading: true } as UpdaterState;
    this.notify();

    try {
      // 1. On native Tauri platforms, trigger browser / system downloader
      await core.openExternal({ url }).catch(() => undefined);

      // 2. Also trigger a web DOM download link if supported in the environment
      if (typeof document !== 'undefined') {
        const link = document.createElement('a');
        link.href = url;
        link.download = assetName;
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
        document.body.appendChild(link);
        link.click();
        document.body.removeChild(link);
      }

      this._state = {
        ...this._state,
        status: 'ready',
        dismissed: false,
      };
      this.notify();
    } catch (err: unknown) {
      this._state = {
        ...this._state,
        status: 'error',
        error: err instanceof Error ? err.message : 'Download failed',
      };
      this.notify();
    }
  }

  /** Checks GitHub Releases for an update and downloads automatically if autoDownload is true. */
  async checkForUpdates(opts: { autoDownload?: boolean; silent?: boolean } = {}): Promise<void> {
    const { autoDownload = true, silent = false } = opts;

    let appInfo = { version: '0.1.0', platform: 'macos' as Platform };
    try {
      const info = await core.appInfo();
      appInfo = { version: info.version, platform: info.platform };
    } catch {
      // Keep defaults
    }

    this._state = {
      ...this._state,
      status: 'checking',
      currentVersion: appInfo.version,
      error: undefined,
    };
    this.notify();

    try {
      const res = await fetch(`https://api.github.com/repos/${GITHUB_REPO}/releases/latest`, {
        headers: { Accept: 'application/vnd.github.v3+json' },
      });

      if (!res.ok) {
        if (res.status === 404) {
          // No release published yet
          this._state = {
            ...this._state,
            status: 'up-to-date',
            lastChecked: Date.now(),
          };
          this.notify();
          return;
        }
        throw new Error(`GitHub returned HTTP ${res.status}`);
      }

      const release = (await res.json()) as GitHubRelease;
      const latest = release.tag_name || '';
      const hasNewer = this.compareVersions(latest, appInfo.version) > 0;

      if (!hasNewer) {
        this._state = {
          ...this._state,
          status: 'up-to-date',
          latestVersion: latest,
          lastChecked: Date.now(),
        };
        this.notify();
        return;
      }

      const asset = this.pickAsset(release.assets, appInfo.platform);
      const downloadUrl = asset ? asset.browser_download_url : release.html_url;
      const assetName = asset ? asset.name : `${release.tag_name}.zip`;

      this._state = {
        ...this._state,
        status: 'available',
        latestVersion: latest,
        releaseNotes: release.body || '',
        downloadUrl,
        assetName,
        lastChecked: Date.now(),
        dismissed: false,
      };
      this.notify();

      if (autoDownload && downloadUrl) {
        await this.download(downloadUrl, assetName);
      }
    } catch (err: unknown) {
      if (!silent) {
        this._state = {
          ...this._state,
          status: 'error',
          error: err instanceof Error ? err.message : 'Could not check for updates',
        };
        this.notify();
      } else {
        this._state = {
          ...this._state,
          status: 'idle',
        };
        this.notify();
      }
    }
  }
}

export const updater = new UpdaterStore();
