<script lang="ts">
  import Button from '../../lib/components/Button.svelte';
  import StatusStrip from '../../lib/components/StatusStrip.svelte';
  import TextField from '../../lib/components/TextField.svelte';
  import { haptic } from '../../lib/haptics';
  import { qrMatrix } from '../login/qr';
  import { createSyncBundle, restoreSyncBundle } from './sync';

  type Mode = 'export' | 'import';
  let mode = $state<Mode>('export');

  // Export state
  let exportPin = $state(generatePin());
  let exportBundle = $state('');
  let exportQr = $state<ReturnType<typeof qrMatrix> | null>(null);
  let exportCopied = $state(false);
  let exportBusy = $state(false);
  let exportError = $state('');

  // Import state
  let importInput = $state('');
  let importPin = $state('');
  let importBusy = $state(false);
  let importStatus = $state<{ tone: 'info' | 'warn'; msg: string } | null>(null);

  function generatePin(): string {
    const n = Math.floor(100000 + Math.random() * 900000);
    return String(n);
  }

  async function handleExport(): Promise<void> {
    if (!exportPin.trim()) return;
    exportBusy = true;
    exportError = '';
    exportBundle = '';
    exportQr = null;
    try {
      const bundle = await createSyncBundle(exportPin.trim());
      exportBundle = bundle;
      try {
        exportQr = qrMatrix(bundle);
      } catch {
        exportQr = null;
      }
      haptic('selection');
    } catch (err: unknown) {
      exportError = err instanceof Error ? err.message : 'Failed to create sync bundle';
      haptic('warning');
    } finally {
      exportBusy = false;
    }
  }

  async function copyBundle(): Promise<void> {
    if (!exportBundle) return;
    try {
      await navigator.clipboard.writeText(exportBundle);
      exportCopied = true;
      haptic('selection');
      setTimeout(() => {
        exportCopied = false;
      }, 2000);
    } catch {
      // Ignore clipboard error
    }
  }

  function downloadBundle(): void {
    if (!exportBundle) return;
    const blob = new Blob([exportBundle], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `chirper-sync-${new Date().toISOString().slice(0, 10)}.bundle`;
    a.click();
    URL.revokeObjectURL(url);
    haptic('selection');
  }

  async function handleImport(): Promise<void> {
    if (!importInput.trim() || !importPin.trim()) {
      importStatus = { tone: 'warn', msg: 'Please enter both the sync bundle and PIN.' };
      return;
    }
    importBusy = true;
    importStatus = null;
    try {
      const res = await restoreSyncBundle(importInput.trim(), importPin.trim());
      importStatus = {
        tone: 'info',
        msg: `Sync restored successfully! Applied preferences and ${res.accountsCount} accounts (${res.chatsCount} chat states).`,
      };
      haptic('selection');
    } catch (err: unknown) {
      importStatus = {
        tone: 'warn',
        msg: err instanceof Error ? err.message : 'Decryption failed. Check the bundle and PIN.',
      };
      haptic('warning');
    } finally {
      importBusy = false;
    }
  }

  function handleFileLoad(event: Event): void {
    const input = event.target as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = (e) => {
      importInput = (e.target?.result as string) ?? '';
    };
    reader.readAsText(file);
  }
</script>

<div class="sync-section">
  <div class="mode-tabs">
    <button
      type="button"
      class="tab-btn"
      class:active={mode === 'export'}
      onclick={() => {
        mode = 'export';
        haptic('selection');
      }}
    >
      Export & Share
    </button>
    <button
      type="button"
      class="tab-btn"
      class:active={mode === 'import'}
      onclick={() => {
        mode = 'import';
        haptic('selection');
      }}
    >
      Import & Restore
    </button>
  </div>

  <p class="t-label t-muted info-text">
    Serverless cross-platform sync transfers your settings, chat organization, and account configs directly between devices. Chirper does not route your private data through any central cloud server.
  </p>

  {#if mode === 'export'}
    <div class="card">
      <div class="field-wrap">
        <TextField
          label="Passcode / 6-digit PIN"
          placeholder="e.g. 123456"
          bind:value={exportPin}
          disabled={exportBusy}
        />
        <p class="t-caption t-muted">This passcode encrypts the bundle so only someone with this code can import it.</p>
      </div>

      <div class="actions">
        <Button variant="primary" disabled={exportBusy || !exportPin.trim()} onclick={() => void handleExport()}>
          {exportBusy ? 'Encrypting…' : 'Generate Encrypted Bundle'}
        </Button>
      </div>

      {#if exportError}
        <StatusStrip tone="warn" message={exportError} />
      {/if}

      {#if exportBundle}
        <div class="bundle-result">
          {#if exportQr}
            <div class="qr-box" role="img" aria-label="Sync bundle QR code">
              <svg viewBox="-2 -2 {exportQr.size + 4} {exportQr.size + 4}" shape-rendering="crispEdges">
                <path d={exportQr.d} fill="#000000" />
              </svg>
            </div>
            <p class="t-caption t-muted center">Scan this QR code from Chirper on another device to restore.</p>
          {/if}

          <div class="export-actions">
            <Button variant="quiet" onclick={() => void copyBundle()}>
              {exportCopied ? 'Copied code!' : 'Copy sync code'}
            </Button>
            <Button variant="quiet" onclick={downloadBundle}>
              Save bundle file
            </Button>
          </div>
        </div>
      {/if}
    </div>
  {:else}
    <div class="card">
      <div class="field-wrap">
        <label for="sync-code-area" class="t-caption t-muted">Paste sync bundle code</label>
        <textarea
          id="sync-code-area"
          class="code-area"
          rows={3}
          placeholder="CHIRPER_SYNC_V1:... or paste file contents"
          bind:value={importInput}
          disabled={importBusy}
        ></textarea>
      </div>

      <div class="file-picker-wrap">
        <label class="file-label">
          <input type="file" accept=".bundle,.txt,.json" onchange={handleFileLoad} disabled={importBusy} />
          <span class="file-btn">Choose file…</span>
        </label>
      </div>

      <div class="field-wrap">
        <TextField
          label="Decryption Passcode / PIN"
          type="password"
          placeholder="Enter the passcode from the export device"
          bind:value={importPin}
          disabled={importBusy}
          onsubmit={() => void handleImport()}
        />
      </div>

      <div class="actions">
        <Button
          variant="primary"
          disabled={importBusy || !importInput.trim() || !importPin.trim()}
          onclick={() => void handleImport()}
        >
          {importBusy ? 'Decrypting…' : 'Restore from Bundle'}
        </Button>
      </div>

      {#if importStatus}
        <StatusStrip tone={importStatus.tone} message={importStatus.msg} />
      {/if}
    </div>
  {/if}
</div>

<style>
  .sync-section {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
    padding: var(--s-3) var(--gutter);
  }

  .mode-tabs {
    display: flex;
    background: var(--bg-hover);
    border-radius: var(--r-md);
    padding: 3px;
    gap: 4px;
  }

  .tab-btn {
    flex: 1;
    border: none;
    background: transparent;
    color: var(--on-card-muted);
    font-size: 14px;
    font-weight: 600;
    padding: 8px 12px;
    border-radius: var(--r-sm);
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }

  .tab-btn.active {
    background: var(--bg-card);
    color: var(--on-card);
    box-shadow: var(--elev-1);
  }

  .info-text {
    line-height: 1.4;
  }

  .card {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }

  .field-wrap {
    display: flex;
    flex-direction: column;
    gap: var(--s-1);
  }

  .code-area {
    width: 100%;
    padding: var(--s-2);
    border-radius: var(--r-md);
    background: var(--bg-subtle, rgba(255, 255, 255, 0.04));
    border: 1px solid var(--border-subtle, rgba(255, 255, 255, 0.1));
    color: var(--on-card);
    font-family: monospace;
    font-size: 12px;
    resize: vertical;
    box-sizing: border-box;
  }

  .file-picker-wrap {
    display: flex;
    align-items: center;
  }

  .file-label input[type='file'] {
    display: none;
  }

  .file-btn {
    display: inline-block;
    font-size: 13px;
    font-weight: 500;
    color: var(--cat-chats, #6e82fe);
    cursor: pointer;
    text-decoration: underline;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }

  .bundle-result {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--s-2);
    padding: var(--s-3);
    background: var(--bg-hover);
    border-radius: var(--r-lg);
  }

  .qr-box {
    width: min(60vw, 220px);
    height: min(60vw, 220px);
    padding: var(--s-2);
    background: #ffffff;
    border-radius: var(--r-md);
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .qr-box svg {
    width: 100%;
    height: 100%;
  }

  .export-actions {
    display: flex;
    gap: var(--s-2);
    flex-wrap: wrap;
    justify-content: center;
  }

  .center {
    text-align: center;
  }
</style>
