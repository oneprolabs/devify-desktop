import { useCallback, useState, type MutableRefObject } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { checkWeChatStep, type StepContext, type WeChatAction } from '../wizard/wechatStepGuard';
import type { Account, WechatEnv } from './nativeTypes';
import type { ConnectionFeedback } from './connectionTypes';
import { useWeChatKeys } from './useWeChatKeys';
import { useWeChatCapture } from './useWeChatCapture';
import { useWeChatDecryption } from './useWeChatDecryption';

type ConnectionOptions = ConnectionFeedback & {
  account: Account | null;
  accountId: string;
  accountSwitching: boolean;
  mode: 'chat' | 'settings';
  wizard: boolean;
  wizardStep: string;
  wizardManual: boolean;
  recoveringKeys?: boolean;
  onWizardStep?: (step: string) => void;
  loadSessions: (id: string) => Promise<unknown>;
  resetReader: () => void;
  clearMessageCache: () => void;
  clearing: MutableRefObject<boolean>;
};

/** Connection state is owned here; chat data and account selection remain in the reader. */
export function useWeChatConnection(options: ConnectionOptions) {
  const { account, accountId, mode, wizardManual } = options;
  const [env, setEnv] = useState<WechatEnv | null>(null);
  const [forceUnlock, setForceUnlock] = useState(mode === 'settings');
  const keys = useWeChatKeys(options);
  // Recovery requires a new validation for this account; a key listed in the
  // store or an existing decrypted copy only proves that it worked before.
  const verifiedImport = keys.keyImport?.accounts.some(match => match.account === accountId && match.matched + match.already > 0);
  const reuseSavedConnection = !options.recoveringKeys || Boolean(verifiedImport);
  const capture = useWeChatCapture({ ...options, ...keys });
  const decryption = useWeChatDecryption({
    ...options,
    refreshKeys: keys.refreshKeys,
    resetKeys: () => { keys.resetResult(); keys.clearFreshSource(); },
    requireKeys: () => { setForceUnlock(true); keys.setShowKeyGuide(true); }
  });

  const initialize = useCallback(async () => {
    try {
      setEnv(await invoke<WechatEnv>('wechat_env'));
      await keys.refreshSavedKeySource();
    } catch { /* not fatal */ }
  }, [keys.refreshSavedKeySource]);

  const resetAccountResult = useCallback(() => {
    decryption.resetResult();
    keys.resetResult();
  }, [decryption.resetResult, keys.resetResult]);

  const checkStep = (action: WeChatAction, context: StepContext, accountExists: () => Promise<boolean>) => checkWeChatStep(action, {
    ...context, accountId, transactionId: capture.captureStatus?.transaction_id ?? null,
    manual: wizardManual, reuseConnection: reuseSavedConnection && (keys.keyedCount > 0 || Boolean(keys.savedKeySource?.exists)),
    capture: async () => { const status = await capture.refreshCaptureStatus(); if(!status) throw new Error('连接状态检查未完成'); return status; },
    accountExists,
    keys: () => keys.checkKeys(accountId),
    source: async () => { const result = await keys.refreshSavedKeySource(); if(!result) throw new Error('连接文件检查未完成'); return result; },
    chats: async () => { await invoke('chat_sessions', { accountId }); }
  });

  const checkConnectionResult = async () => {
    await Promise.all([capture.refreshCaptureStatus(), keys.refreshKeys(), keys.refreshSavedKeySource()]);
  };

  return {
    account, env, forceUnlock, setForceUnlock, keys, capture, decryption, reuseSavedConnection,
    initialize, resetAccountResult, checkStep, checkConnectionResult
  };
}

export type WeChatConnection = ReturnType<typeof useWeChatConnection>;
