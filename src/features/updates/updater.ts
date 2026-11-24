import { t, tf, ctf } from '../../shared/i18n/locale';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { useSyncExternalStore } from 'react';
import { DEVELOPMENT_APP } from '../../shared/api/buildFlavor';

export const RELEASE_URL = 'https://github.com/oneprolabs/devify-desktop-releases/releases/tag/alpha';
type Status = 'disabled' | 'idle' | 'checking' | 'latest' | 'available' | 'downloading' | 'downloaded' | 'installing' | 'installed' | 'error';
type Snapshot = { status: Status; version?: string; currentVersion?: string; notes?: string; progress?: number; error?: string; blockers?: string[] };
let snapshot: Snapshot = { status: DEVELOPMENT_APP ? 'disabled' : 'idle' };
let update: Update | null = null;
let checkInFlight: Promise<void> | undefined;
let refreshGeneration = 0;
const listeners = new Set<() => void>();
function setSnapshot(next: Snapshot) { snapshot = next; listeners.forEach(listener => listener()); }

/**
 * The updater plugin surfaces transport failures verbatim ("Could not fetch a valid
 * release JSON from the remote"). Translate the common cases into an actionable
 * sentence instead of showing the raw plugin error.
 */
function friendlyUpdateError(error: unknown): string {
  const text = String(error);
  if (/valid release json|404|not found/i.test(text)) {
    return t("更新服务暂不可用：发布仓库还没有可用的版本清单，请稍后重试。");
  }
  if (/timed? ?out|timeout|connect|network|dns|failed to fetch|error sending request/i.test(text)) {
    return t("无法连接更新服务器，请检查网络，或在系统设置中开启代理后重试。");
  }
  return tf("检查更新失败：{0}", text);
}
export function useUpdater() { return useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener); }, () => snapshot, () => snapshot); }

export function checkForUpdates(silent = false): Promise<void> {
  if (DEVELOPMENT_APP) return Promise.resolve();
  if (checkInFlight) return checkInFlight;
  if (!isTauri() || ['downloading', 'installing', 'downloaded', 'installed'].includes(snapshot.status)) return Promise.resolve();
  checkInFlight = refreshUpdate(silent).finally(() => { checkInFlight = undefined; });
  return checkInFlight;
}

async function refreshUpdate(silent: boolean) {
  const generation = ++refreshGeneration;
  const previous = update;
  const previousSnapshot = snapshot;
  // Keep an already discovered offer visible while a background refresh runs. The
  // rail action remains available, and downloadUpdate no longer waits on this
  // refresh before starting the download.
  if (!previous) setSnapshot({ status: 'checking' });
  try {
    // The manifest lives on github.com; the plugin uses the system proxy settings.
    const next = await check({ timeout: 30000, headers: { 'Cache-Control': 'no-cache' } });
    // A download may have started while this background refresh was in flight.
    // Its result is stale and must not replace the download state or close the
    // Update object that the download is using.
    if (generation !== refreshGeneration || ['downloading', 'installing', 'downloaded', 'installed'].includes(snapshot.status)) {
      if (next && next !== update) await next.close().catch(() => {});
      return;
    }
    update = next;
    setSnapshot(next ? { status: 'available', version: next.version, currentVersion: next.currentVersion, notes: next.body } : { status: 'latest' });
    if (previous) await previous.close().catch(() => {});
  } catch (error) {
    if (generation !== refreshGeneration || ['downloading', 'installing', 'downloaded', 'installed'].includes(snapshot.status)) return;
    if (previous) {
      // A transient refresh failure must not discard a valid offer. Keep it
      // actionable and expose the failure only for an explicit check.
      setSnapshot({
        status: 'available',
        version: previousSnapshot.version ?? previous.version,
        currentVersion: previousSnapshot.currentVersion ?? previous.currentVersion,
        notes: previousSnapshot.notes ?? previous.body,
        ...(silent ? {} : { error: friendlyUpdateError(error) }),
      });
    } else {
      setSnapshot(silent ? { status: 'idle' } : { status: 'error', error: friendlyUpdateError(error) });
    }
  }
}

export async function downloadUpdate() {
  if (DEVELOPMENT_APP) return;
  if (snapshot.status !== 'available') return;
  // The available Update object was already verified by checkForUpdates. Do not
  // perform another network check here: a slow or unavailable manifest used to
  // leave the button apparently inert even though the current offer was ready to
  // download.
  if (!update) return;
  // Invalidate any background manifest check that started before this click. Its
  // result must not overwrite the download state or close the selected object.
  refreshGeneration += 1;
  const selected = update;
  const base = { version: selected.version, currentVersion: selected.currentVersion, notes: selected.body };
  setSnapshot({ status: 'downloading', ...base, progress: 0 });
  let received = 0;
  let total: number | undefined;
  try {
    await selected.download(event => {
      if (event.event === 'Started') total = event.data.contentLength;
      if (event.event === 'Progress') received += event.data.chunkLength;
      setSnapshot({ status: 'downloading', ...base, progress: total ? Math.min(100, Math.round(received * 100 / total)) : undefined });
    }, { timeout: 600000 });
    setSnapshot({ status: 'downloaded', ...base, progress: 100 });
  } catch (error) {
    setSnapshot({ status: 'available', ...base, error: tf("下载失败：{0}", String(error)) });
  }
}

/**
 * Install the downloaded update. The app restarts, so it refuses while an upload is in
 * flight: the in-flight request is dropped and its batch has to be retried afterwards.
 * Instead of dead-ending on an opaque error, the first attempt reports the busy accounts
 * (`blockers`) and the caller can confirm with `installUpdate(true)`.
 */
export async function installUpdate(force = false) {
  if (DEVELOPMENT_APP) return;
  if (!update || !['downloaded', 'installed'].includes(snapshot.status)) return;
  const selected = update;
  const base = { version: selected.version, currentVersion: selected.currentVersion, notes: selected.body, progress: 100 };
  const alreadyInstalled = snapshot.status === 'installed';
  if (!force) {
    try {
      const accounts = await invoke<string[]>('update_upload_accounts');
      if (accounts.length) { setSnapshot({ status: snapshot.status, ...base, blockers: accounts }); return; }
    } catch {
      // The install command re-checks the gate, so a failed probe is not fatal.
    }
  }
  let locked = false;
  let installed = alreadyInstalled;
  setSnapshot({ status: 'installing', ...base });
  try {
    await invoke('update_begin_install', { force });
    locked = true;
    if (!alreadyInstalled) { await selected.install(); installed = true; }
    await relaunch();
  } catch (error) {
    if (locked) await invoke('update_cancel_install').catch(() => {});
    setSnapshot({ status: installed ? 'installed' : 'downloaded', ...base, error: tf("{0}：{1}", installed ? '重启失败' : '安装失败', String(error)) });
  }
}

/** Keep the downloaded update but hide the "upload in progress" confirmation. */
export function dismissInstallBlockers() {
  if (snapshot.blockers) setSnapshot({ ...snapshot, blockers: undefined });
}
