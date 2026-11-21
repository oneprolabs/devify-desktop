import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { t } from '../../shared/i18n/locale';
import { defaultSettings, mergeSettings, requireSettingsRevision, settingsSaveError, type AppSettings } from '../settings/settingsModel';
import type { AccessStatus, AgentInfo } from './agentCatalog';

/** One settings owner and explicit refreshes; no discovery/status interval. */
export function useAgentIntegration(canAccess: boolean) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [ready, setReady] = useState(false);
  const [saving, setSaving] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [access, setAccess] = useState<AccessStatus | null>(null);
  const [error, setError] = useState('');
  const [scanError, setScanError] = useState('');
  const [accessError, setAccessError] = useState('');
  const [notice, setNotice] = useState('');
  const current = useRef(settings);
  const active = useRef(false);
  const savingLock = useRef(false);
  const scanningLock = useRef(false);
  const accessEpoch = useRef(0);
  const loadEpoch = useRef(0);
  const accessEnabled = useRef(canAccess);
  const lastAccess = useRef(canAccess);
  accessEnabled.current = canAccess;

  const refreshAccess = useCallback(async () => {
    if (!accessEnabled.current) return;
    const epoch = ++accessEpoch.current;
    try {
      const next = await invoke<AccessStatus>('agent_access_status');
      if (active.current && epoch === accessEpoch.current) { setAccess(next); setAccessError(''); }
    } catch (reason) {
      if (active.current && epoch === accessEpoch.current) setAccessError(String(reason));
    }
  }, []);

  const scan = useCallback(async () => {
    if (scanningLock.current) return;
    scanningLock.current = true; setScanning(true); setScanError('');
    try {
      // Access discovery and CLI version probes are independent.
      const [result] = await Promise.allSettled([invoke<AgentInfo[]>('agent_scan'), refreshAccess()]);
      if (!active.current) return;
      if (result.status === 'fulfilled') setAgents(result.value);
      else setScanError(String(result.reason)); // Preserve the last successful scan on failure.
    } finally {
      scanningLock.current = false;
      if (active.current) setScanning(false);
    }
  }, [refreshAccess]);

  const reload = useCallback(async () => {
    const request = ++loadEpoch.current;
    try {
      const next = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_get')));
      if (active.current && request === loadEpoch.current) { current.current = next; setSettings(next); setReady(true); setError(''); }
    } catch (reason) { if (active.current && request === loadEpoch.current) setError(String(reason)); }
  }, []);

  useEffect(() => {
    active.current = true;
    void reload(); void scan();
    return () => { active.current = false; accessEpoch.current += 1; loadEpoch.current += 1; };
  }, [reload, scan]);

  useEffect(() => {
    if (lastAccess.current === canAccess) return;
    lastAccess.current = canAccess;
    if (canAccess) void refreshAccess();
    else { accessEpoch.current += 1; setAccess(null); setAccessError(''); }
  }, [canAccess, refreshAccess]);

  const save = async (edit: (value: AppSettings) => AppSettings): Promise<boolean> => {
    if (!ready || savingLock.current) return false;
    savingLock.current = true; setSaving(true); setError(''); setNotice('');
    try {
      const submitted = edit(current.current);
      const next = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: submitted })));
      // A confirmed native save also affects readers that remain mounted after navigation.
      window.dispatchEvent(new Event('agent-settings-changed'));
      if (!active.current) return false;
      current.current = next; setSettings(next); setNotice(t('已保存'));
      return true;
    } catch (reason) { if (active.current) setError(settingsSaveError(reason)); return false; }
    finally { savingLock.current = false; if (active.current) setSaving(false); }
  };
  return { settings, ready, saving, scanning, agents, access, error, scanError, accessError, notice, save, scan, reload, refreshAccess };
}
