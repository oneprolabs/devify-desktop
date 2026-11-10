import { invoke, isTauri } from '@tauri-apps/api/core';
import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { useSyncExternalStore } from 'react';

export const RELEASE_URL = 'https://github.com/oneprolabs/devify-desktop/releases/tag/alpha';
type Status = 'idle' | 'checking' | 'latest' | 'available' | 'downloading' | 'downloaded' | 'installing' | 'installed' | 'error';
type Snapshot = { status: Status; version?: string; notes?: string; progress?: number; error?: string };
let snapshot: Snapshot = { status: 'idle' };
let update: Update | null = null;
const listeners = new Set<() => void>();
function setSnapshot(next: Snapshot) { snapshot = next; listeners.forEach(listener => listener()); }
export function useUpdater() { return useSyncExternalStore(listener => { listeners.add(listener); return () => listeners.delete(listener); }, () => snapshot, () => snapshot); }

export async function checkForUpdates(silent = false) {
  if (!isTauri() || ['checking', 'downloading', 'installing', 'downloaded', 'installed'].includes(snapshot.status)) return;
  setSnapshot({ status: 'checking' });
  try {
    const next = await check({ timeout: 15000 });
    if (update) await update.close();
    update = next;
    setSnapshot(next ? { status: 'available', version: next.version, notes: next.body } : { status: 'latest' });
  } catch (error) {
    setSnapshot(silent ? { status: 'idle' } : { status: 'error', error: String(error) });
  }
}

export async function downloadUpdate() {
  if (!update || snapshot.status !== 'available') return;
  const selected = update;
  const base = { version: selected.version, notes: selected.body };
  setSnapshot({ status: 'downloading', ...base, progress: 0 });
  let received = 0;
  let total: number | undefined;
  try {
    await selected.download(event => {
      if (event.event === 'Started') total = event.data.contentLength;
      if (event.event === 'Progress') received += event.data.chunkLength;
      setSnapshot({ status: 'downloading', ...base, progress: total ? Math.min(100, Math.round(received * 100 / total)) : undefined });
    });
    setSnapshot({ status: 'downloaded', ...base, progress: 100 });
  } catch (error) {
    setSnapshot({ status: 'available', ...base, error: `下载失败：${String(error)}` });
  }
}

export async function installUpdate() {
  if (!update || !['downloaded', 'installed'].includes(snapshot.status)) return;
  const selected = update;
  const base = { version: selected.version, notes: selected.body, progress: 100 };
  const alreadyInstalled = snapshot.status === 'installed';
  let locked = false;
  let installed = alreadyInstalled;
  setSnapshot({ status: 'installing', ...base });
  try {
    await invoke('update_begin_install');
    locked = true;
    if (!alreadyInstalled) { await selected.install(); installed = true; }
    await relaunch();
  } catch (error) {
    if (locked) await invoke('update_cancel_install').catch(() => {});
    setSnapshot({ status: installed ? 'installed' : 'downloaded', ...base, error: `${installed ? '重启失败' : '安装失败'}：${String(error)}` });
  }
}
