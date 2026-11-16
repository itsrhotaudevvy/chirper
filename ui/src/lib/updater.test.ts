import { describe, it, expect } from 'vitest';
import { updater, type GitHubReleaseAsset } from './updater';

describe('updater', () => {
  it('compares semver versions correctly', () => {
    expect(updater.compareVersions('0.1.1', '0.1.0')).toBe(1);
    expect(updater.compareVersions('v0.2.0', '0.1.9')).toBe(1);
    expect(updater.compareVersions('0.1.0', '0.1.0')).toBe(0);
    expect(updater.compareVersions('v0.1.0', '0.1.0')).toBe(0);
    expect(updater.compareVersions('0.1.0', '0.1.1')).toBe(-1);
    expect(updater.compareVersions('0.1.0', '1.0.0')).toBe(-1);
    expect(updater.compareVersions('1.2.3', '1.2.4')).toBe(-1);
  });

  it('picks appropriate assets for platforms', () => {
    const assets: GitHubReleaseAsset[] = [
      { name: 'chirper-android-arm64.apk', browser_download_url: 'https://example.com/arm64.apk', size: 10, content_type: '' },
      { name: 'chirper-android-universal.apk', browser_download_url: 'https://example.com/universal.apk', size: 20, content_type: '' },
      { name: 'Chirper_0.1.0_aarch64.dmg', browser_download_url: 'https://example.com/mac.dmg', size: 30, content_type: '' },
      { name: 'Chirper_0.1.0_x64-setup.exe', browser_download_url: 'https://example.com/win.exe', size: 40, content_type: '' },
      { name: 'chirper_0.1.0_amd64.AppImage', browser_download_url: 'https://example.com/linux.AppImage', size: 50, content_type: '' },
      { name: 'Chirper.ipa', browser_download_url: 'https://example.com/ios.ipa', size: 60, content_type: '' },
    ];

    expect(updater.pickAsset(assets, 'android')?.name).toBe('chirper-android-universal.apk');
    expect(updater.pickAsset(assets, 'macos')?.name).toBe('Chirper_0.1.0_aarch64.dmg');
    expect(updater.pickAsset(assets, 'windows')?.name).toBe('Chirper_0.1.0_x64-setup.exe');
    expect(updater.pickAsset(assets, 'linux')?.name).toBe('chirper_0.1.0_amd64.AppImage');
    expect(updater.pickAsset(assets, 'ios')?.name).toBe('Chirper.ipa');
  });

  it('updates state and notifies subscribers on dismiss', () => {
    updater.dismiss();
    expect(updater.state.dismissed).toBe(true);
  });
});
