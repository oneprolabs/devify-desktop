import { useCallback, useState, type MutableRefObject } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t, tf } from '../../../shared/i18n/locale';
import type { Account, DecryptOutcome } from './nativeTypes';
import type { ConnectionFeedback } from './connectionTypes';
import { syncWechatForRefresh } from './refresh';

export function useWeChatDecryption({ account, accountSwitching, clearing, resetReader, clearMessageCache, resetKeys, refreshKeys, loadSessions, requireKeys, setNotice, setError, setBusy, recoveringKeys }: ConnectionFeedback & {
  account: Account | null; accountSwitching: boolean; clearing: MutableRefObject<boolean>;
  resetReader: () => void; clearMessageCache: () => void; resetKeys: () => void;
  refreshKeys: () => Promise<void>; loadSessions: (id: string) => Promise<unknown>;
  requireKeys: () => void;
  recoveringKeys?: boolean;
}) {
  const [applyWal, setApplyWal] = useState(true);
  const [onlyChat, setOnlyChat] = useState(true);

  const [decrypting, setDecrypting] = useState(false);
  const [outcomes, setOutcomes] = useState<DecryptOutcome[]>([]);
  const [recovered, setRecovered] = useState(false);
  const [showLog, setShowLog] = useState(false);

  // `window.confirm` is not surfaced by the macOS webview (it silently returns false), so a
  // destructive action needs its own confirmation step in the page instead of a native dialog.
  const [clearConfirm, setClearConfirm] = useState(false);

  const clearKeys = async () => {
    if(!account) return;
    setClearConfirm(false);
    setError('');
    setBusy('import');
    // Same guard as `clearCache`: live sync must not re-create the copy mid-request.
    clearing.current = true;
    try {
      const text = await invoke<string>('wechat_clear_keys', { accountId: account.id });
      setNotice(text);
      resetKeys();
      setOutcomes([]);
      resetReader();
      await refreshKeys();
    } catch(e) {
      setError(String(e));
    } finally {
      window.setTimeout(() => {
        clearing.current = false;
      }, 0);
      setBusy(null);
    }
  };

  const clearCache = async () => {
    if(!account) return;
    setError('');
    // Live sync runs on an interval; without this guard it can re-create the copy while the
    // clear request is in flight, making the action look like a no-op.
    clearing.current = true;
    try {
      const text = await invoke<string>('wechat_clear_cache', { accountId: account.id });
      clearMessageCache();
      setNotice(text);
      setOutcomes([]);
      resetReader();
      await refreshKeys();
    } catch(e) {
      setError(String(e));
    } finally {
      // Release after the state updates have been committed so the next interval tick does
      // not race the switch back to the onboarding view.
      window.setTimeout(() => {
        clearing.current = false;
      }, 0);
    }
  };

  const decrypt = async () => {
    if(!account || accountSwitching || decrypting) return;
    setError('');
    setNotice('');
    setDecrypting(true);
    try {
      if (recoveringKeys) {
        const issue = await syncWechatForRefresh(account.id);
        if (issue) {
          if (issue.needsKeys) resetKeys();
          setError(issue.message);
          return;
        }
        setRecovered(true);
        return;
      }
      const result = await invoke<DecryptOutcome[]>('wechat_decrypt', {
        accountId: account.id,
        applyWal,
        onlyChat
      });
      setOutcomes(result);
      const ok = result.filter(o => o.status === 'ok' || o.status === 'cached').length;
      const failed = result.length - ok;
      setNotice(failed ? tf("解密完成：{0} 个数据库可用，{1} 个失败或缺少密钥。", ok, failed) : tf("解密完成：{0} 个数据库可用。", ok));
      if(ok > 0) {
        await loadSessions(account.id);
      } else if(result.length > 0) {
        // Nothing decrypted at all: the most likely cause is a key for another account or a
        // rotated passphrase, so surface the acquisition guide instead of a silent failure.
        requireKeys();
        setError(t("没有任何数据库解密成功，通常是密钥与当前账号不匹配或已轮换。请重新捕获口令后再导入。"));
      }
    } catch(e) {
      setError(String(e));
    } finally {
      setDecrypting(false);
    }
  };

  const resetResult = useCallback(() => { setOutcomes([]); setRecovered(false); }, []);
  const decrypted = recoveringKeys ? recovered : outcomes.some(item => item.rel === 'session/session.db' && (item.status === 'ok' || item.status === 'cached')) &&
    outcomes.some(item => /^message\/message_\d+\.db$/.test(item.rel) && (item.status === 'ok' || item.status === 'cached'));

  return {
    applyWal, setApplyWal, onlyChat, setOnlyChat, decrypting, outcomes, showLog, setShowLog,
    clearConfirm, setClearConfirm, clearKeys, clearCache, decrypt, resetResult, decrypted
  };
}

export type WeChatDecryption = ReturnType<typeof useWeChatDecryption>;
