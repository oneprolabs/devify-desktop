import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t, tf } from '../../../shared/i18n/locale';
import type { Account, KeyStatus, SavedKeySource, KeyImportResult } from './nativeTypes';
import type { ConnectionFeedback } from './connectionTypes';

export function useWeChatKeys({ account, wizard, recoveringKeys, setNotice, setError, setBusy }: ConnectionFeedback & {
  account: Account | null; wizard: boolean; recoveringKeys?: boolean;
}) {
  const [keyStatus, setKeyStatus] = useState<KeyStatus | null>(null);
  const [savedKeySource, setSavedKeySource] = useState<SavedKeySource | null>(null);
  const [keyText, setKeyText] = useState('');
  const [showKeyGuide, setShowKeyGuide] = useState(false);
  const [windowsScanBusy, setWindowsScanBusy] = useState(false);
  useEffect(() => { if(wizard) setShowKeyGuide(true); }, [wizard]);
  const [keyImport, setKeyImport] = useState<KeyImportResult | null>(null);
  const [freshSource, setFreshSource] = useState(false);
  const [fixingPermissions, setFixingPermissions] = useState(false);
  const importScope = { accountId: account?.id ?? null, ...(recoveringKeys && { targetOnly: true }) };
  const sourceSeen = useRef<{ exists: boolean; modifiedAt: number | null } | null>(null);
  const encryptedDbs = useMemo(() => account?.dbs.filter(db => db.encrypted) ?? [], [account]);
  const keyedCount = useMemo(
    () => encryptedDbs.filter(db => keyStatus?.keys.some(k => k.account === account?.id && k.rel === db.rel)).length,
    [encryptedDbs, keyStatus, account],
  );
  const accountMissingKeys = Boolean(account && encryptedDbs.length > 0 && keyedCount < encryptedDbs.length);
  const noKeysYet = Boolean(account && encryptedDbs.length > 0 && keyedCount === 0);
  const keyVerifiedSecs = useMemo(() => {
    if(!account) return null;
    const stamps = (keyStatus?.keys ?? [])
      .filter(key => key.account === account.id && key.verified_secs)
      .map(key => key.verified_secs as number);
    return stamps.length ? Math.max(...stamps) : null;
  }, [keyStatus, account]);
  const keyAgeDays = keyVerifiedSecs
    ? Math.max(0, Math.floor((Date.now() / 1000 - keyVerifiedSecs) / 86400))
    : null;
  const refreshKeys = useCallback(async () => {
    setKeyStatus(await invoke<KeyStatus>('wechat_key_status'));
  }, []);

  const refreshSavedKeySource = useCallback(async () => {
    try {
      const next = await invoke<SavedKeySource>('wechat_key_source_status');
      const seen = sourceSeen.current;
      if(seen && next.exists && (!seen.exists || (next.modified_at ?? 0) > (seen.modifiedAt ?? 0))) {
        setFreshSource(true);
        setNotice(t("检测到新的口令文件，可以导入并校验。"));
      }
      sourceSeen.current = { exists: next.exists, modifiedAt: next.modified_at ?? null };
      setSavedKeySource(next);
      return next;
    } catch {
      /* The probe is advisory: a missing home directory must not block the page. */
      return null;
    }
  }, []);

  useEffect(() => {
    if(noKeysYet) setShowKeyGuide(true);
  }, [account?.id, noKeysYet]);

  // While the guide is open (or before any key exists) watch the extractor's well-known
  // output file. It is a cheap local stat; no chat data and no passphrase is read here.
  useEffect(() => {
    if(wizard || (!showKeyGuide && !noKeysYet)) return;
    void refreshSavedKeySource();
    const timer = window.setInterval(() => void refreshSavedKeySource(), 3000);
    return () => window.clearInterval(timer);
  }, [wizard, showKeyGuide, noKeysYet, refreshSavedKeySource]);

  const applyImportResult = async (result: KeyImportResult) => {
    setKeyImport(result);
    setFreshSource(false);
    setNotice(result.message);
    // Only wipe the box when the pasted material actually stored something; a rejected paste
    // stays visible so it can be corrected instead of retyped from memory.
    if(result.matched > 0) setKeyText('');
    const selected = result.accounts.find(entry => entry.account === account?.id);
    const verified = account
      ? (selected?.matched ?? 0) + (selected?.already ?? 0)
      : result.matched + result.already;
    if (result.unmatched.length) {
      setError(tf("有 {0} 个数据库没有匹配到密钥（微信可能从未打开过它们）。", result.unmatched.length));
    } else if (!verified) {
      setError(result.message);
    }
    await refreshKeys();
  };

  const importKeys = async (file?: File, pastedText = keyText) => {
    setError('');
    setNotice('');
    let text = pastedText;
    if (file) {
      if (file.size > 2_000_000) {
        setError(t("密钥文件过大，请确认选择的是口令或密钥 JSON。"));
        return;
      }
      try {
        text = await file.text();
      } catch {
        setError(t("无法读取密钥文件，请重新选择。"));
        return;
      }
      setKeyText(text);
    }
    if (!text.trim()) {
      setError(t("请粘贴密钥或口令，或选择口令 JSON 文件。"));
      return;
    }
    setBusy('import');
    try {
      const result = await invoke<KeyImportResult>('wechat_import_keys', {
        text,
        ...importScope,
      });
      await applyImportResult(result);
    } catch(e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const scanWindowsKeys = async () => {
    setWindowsScanBusy(true); setError(''); setNotice('');
    try {
      const result = await invoke<KeyImportResult>('wechat_windows_key_scan', importScope);
      setKeyImport(result); await refreshKeys();
      setNotice(result.message || t("Windows 密钥已校验并导入。"));
    } catch(reason) { setError(String(reason)); }
    finally { setWindowsScanBusy(false); }
  };

  const importSavedKey = async () => {
    setError('');
    setNotice('');
    setBusy('import');
    try {
      const result = await invoke<KeyImportResult>('wechat_import_saved_key', importScope);
      await applyImportResult(result);
      await refreshSavedKeySource();
    } catch(e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const fixKeyPermissions = async () => {
    setError('');
    setFixingPermissions(true);
    try {
      const next = await invoke<SavedKeySource>('wechat_fix_key_permissions');
      setSavedKeySource(next);
      setNotice(tf("口令文件权限已改为 {0}，只有当前用户可读。", next.mode ?? '600'));
    } catch(e) {
      setError(String(e));
    } finally {
      setFixingPermissions(false);
    }
  };

  const resetResult = useCallback(() => setKeyImport(null), []);
  const recordCaptureResult = (result: KeyImportResult) => setKeyImport(result);
  const checkKeys = async (accountId: string) => {
    const result = await invoke<KeyStatus>('wechat_key_status');
    setKeyStatus(result);
    return result.keys.filter(key => key.account === accountId).length;
  };

  return {
    keyStatus, savedKeySource, keyText, setKeyText, showKeyGuide, setShowKeyGuide,
    windowsScanBusy, keyImport, freshSource, fixingPermissions,
    encryptedDbs, keyedCount, accountMissingKeys, noKeysYet, keyVerifiedSecs, keyAgeDays,
    refreshKeys, refreshSavedKeySource, importKeys, scanWindowsKeys, importSavedKey,
    clearFreshSource: () => setFreshSource(false),
    fixKeyPermissions, resetResult, recordCaptureResult, checkKeys
  };
}

export type WeChatKeys = ReturnType<typeof useWeChatKeys>;
