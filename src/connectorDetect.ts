import { useCallback, useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

export type DetectState = 'installed' | 'not_installed' | 'untrusted' | 'unsupported' | 'not_required';
export type ConnectorDetect = {
  state: DetectState;
  app_path: string | null;
  version: string | null;
  message: string;
};

/** Re-checked on mount, on window focus and on demand, so a reinstall is picked up without a restart. */
export function useConnectorDetect(kind: string) {
  const [detect, setDetect] = useState<ConnectorDetect | null>(null);
  const [checking, setChecking] = useState(isTauri());
  const recheck = useCallback(async () => {
    if (!isTauri()) return;
    setChecking(true);
    try {
      setDetect(await invoke<ConnectorDetect>('connector_detect', { kind }));
    } catch (reason) {
      setDetect({ state: 'not_installed', app_path: null, version: null, message: `无法检测：${String(reason)}` });
    } finally {
      setChecking(false);
    }
  }, [kind]);
  useEffect(() => {
    void recheck();
    const onFocus = () => { void recheck(); };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [recheck]);
  const usable = detect?.state === 'installed' || detect?.state === 'not_required';
  // An untrusted WeChat bundle is still sufficient for the read-only/import flow. The
  // signature gate remains inside the in-app capture command, which is the only path that
  // temporarily replaces the installed bundle.
  const canProceed = usable || (kind === 'wechat' && detect?.state === 'untrusted');
  return { detect, checking, usable, canProceed, recheck };
}

export const detectTitle: Record<DetectState, string> = {
  installed: '已检测到微信',
  not_installed: '未检测到微信',
  untrusted: '检测到的微信不是官方版本',
  unsupported: '当前系统暂不支持',
  not_required: '无需检测',
};

/** Same wording for another source; `detectTitle` stays the WeChat table used by the wizard. */
export function detectTitleFor(label: string, state: DetectState): string {
  return ({
    installed: `已检测到${label}`,
    not_installed: `未检测到${label}`,
    untrusted: `检测到的${label}不是官方版本`,
    unsupported: '当前系统暂不支持',
    not_required: '无需检测',
  } as Record<DetectState, string>)[state];
}

export type ChatAccount = {
  id: string;
  source?: 'wechat' | 'whatsapp' | string;
  display?: string | null;
  modified_at?: number | null;
  encrypted_count?: number | null;
};

/** Newest active account first; the remembered one wins when it is still present. */
export function currentAccount<T extends { id: string; modified_at?: number | null }>(accounts: T[], remembered?: string | null): T | undefined {
  return accounts.find(account => account.id === remembered)
    ?? [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
}
