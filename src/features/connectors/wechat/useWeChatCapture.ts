import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../../shared/api/desktopInvoke';
import { t, tf } from '../../../shared/i18n/locale';
import { capturePrerequisiteFailure } from '../wizard/wechatStepGuard';
import type { Account, CaptureStatus, CaptureImport, LaunchDiagnosis, PrepareResult, LaunchAttempt, LaunchCheck, PreflightResult, RestoreResult } from './nativeTypes';
import type { ConnectionFeedback } from './connectionTypes';
import type { WeChatKeys } from './useWeChatKeys';

export function useWeChatCapture({ account, wizard, wizardStep, recoveringKeys, onWizardStep, showKeyGuide, noKeysYet, refreshKeys, refreshSavedKeySource, recordCaptureResult, setNotice, setError }: Pick<ConnectionFeedback, 'setNotice' | 'setError'> & Pick<WeChatKeys, 'showKeyGuide' | 'noKeysYet' | 'refreshKeys' | 'refreshSavedKeySource' | 'recordCaptureResult'> & {
  account: Account | null; wizard: boolean; wizardStep: string; recoveringKeys?: boolean;
  onWizardStep?: (step: string) => void;
}) {
  /** In-app capture: environment/state from the backend plus the step currently running. */
  const [captureStatus, setCaptureStatus] = useState<CaptureStatus | null>(null);
  const [captureStatusChecking, setCaptureStatusChecking] = useState(false);
  const [captureStatusError, setCaptureStatusError] = useState('');
  const [recoveryNotice, setRecoveryNotice] = useState('');
  const [recoveryError, setRecoveryError] = useState('');
  const captureStatusRequest = useRef(0);
  const [captureBusy, setCaptureBusy] = useState<
    'prepare' | 'open' | 'confirm' | 'preflight' | 'start' | 'restore' | 'accept-official' | null
  >(null);
  const [captureOutcome, setCaptureOutcome] = useState<CaptureImport | null>(null);
  /** The user has to acknowledge that WeChat will be re-signed and re-logged-in. */
  const [captureRiskAccepted, setCaptureRiskAccepted] = useState(false);
  /** Set when the temporary WeChat did not start, so the guide can explain Gatekeeper rules. */
  const [launchDiagnosis, setLaunchDiagnosis] = useState<LaunchDiagnosis | null>(null);
  /* ── in-app capture ───────────────────────────────────────────────────── */

  /** Backend errors are structured; show the message, not a JSON blob. */
  const captureErrorText = (e: unknown): string => {
    if(typeof e === 'string') return e;
    if(e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message);
    return String(e);
  };

  const refreshCaptureStatus = useCallback(async () => {
    const request = ++captureStatusRequest.current;
    setCaptureStatusChecking(true);
    try {
      const status = await invoke<CaptureStatus>('wechat_capture_status');
      if(request !== captureStatusRequest.current) return status;
      setCaptureStatus(status);
      setCaptureStatusError('');
      return status;
    } catch(reason) {
      if(request === captureStatusRequest.current) setCaptureStatusError(String(reason));
    } finally {
      if(request === captureStatusRequest.current) setCaptureStatusChecking(false);
    }
  }, []);

  useEffect(() => {
    if(wizard || (!showKeyGuide && !noKeysYet)) return;
    void refreshCaptureStatus();
    const timer = window.setInterval(() => void refreshCaptureStatus(), 1500);
    return () => window.clearInterval(timer);
  }, [wizard, showKeyGuide, noKeysYet, refreshCaptureStatus]);

  useEffect(() => {
    if(wizard && wizardStep === 'wizard-permission') void refreshCaptureStatus();
  }, [wizard, wizardStep, refreshCaptureStatus]);

  const captureRunning = captureBusy === 'start' || captureStatus?.stage === 'capturing';
  useEffect(() => {
    if(!wizard || !captureRunning) return;
    const timer = window.setInterval(() => void refreshCaptureStatus(), 1500);
    return () => window.clearInterval(timer);
  }, [wizard, captureRunning, refreshCaptureStatus]);

  /** Verify, back up and temporarily re-sign WeChat. Nothing is launched here. */
  const prepareCapture = async () => {
    setError('');
    setRecoveryNotice('');
    setLaunchDiagnosis(null);
    setNotice(t("正在校验微信签名并建立原版备份，可能需要几分钟，请不要关闭窗口…"));
    setCaptureBusy('prepare');
    try {
      const result = await invoke<PrepareResult>('wechat_capture_prepare');
      const sanitized =
        result.entitlement_mode === 'deep'
          ? t(" 本次临时副本及内部组件会被重签，并以无沙箱模式运行，可能使用不同的数据目录。结束或取消时会恢复原版。")
          : result.entitlement_mode === 'sanitized'
            ? tf(" 这台 Mac 会拒绝 ad-hoc 签名携带腾讯团队的受限权限，本次临时副本已剔除 {0} 项受限权限（{1}），否则微信在启动前就会被系统拒绝；结束或取消时会换回腾讯原版。", result.dropped_entitlements.length, result.dropped_entitlements.join('、'))
            : '';
      setNotice(
        tf("已备份微信 {0}（{1}）并完成临时重签。{2}下一步：打开微信并完整登录要读取的账号。", result.version, result.build, sanitized),
      );
    } catch(e) {
      setError(captureErrorText(e));
      return capturePrerequisiteFailure(e);
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  const openCaptureWeChat = async () => {
    setError('');
    setCaptureBusy('open');
    try {
      const result = await invoke<LaunchAttempt>('wechat_capture_open');
      setLaunchDiagnosis(result.launched ? null : result.diagnosis);
      if(result.launched) {
        setNotice(t("已打开临时微信。请等登录界面出现并完整登录，然后点「我已登录，检查启动」。"));
      } else {
        // The card carries the `open` stderr and the fix, so no error toast that could cover it.
        setNotice(
          result.diagnosis.amfi_rejected
            ? t("这台 Mac 在启动前就拒绝了本次临时签名，无法通过放行解决，请按下面的说明恢复微信。")
            : t("系统没有确认微信启动，请按下面的步骤放行后再试。"),
        );
      }
    } catch(e) {
      setError(captureErrorText(e));
    } finally {
      setCaptureBusy(null);
    }
  };

  const openPrivacySecuritySettings = async () => {
    try {
      await invoke('wechat_open_privacy_security_settings');
      setNotice(t("已打开「隐私与安全性」。请在「安全性」区域找到被阻止的微信，点「仍要打开」。"));
    } catch(e) {
      setError(captureErrorText(e));
    }
  };

  const confirmCaptureLaunch = async () => {
    const transaction = captureStatus?.transaction_id;
    if(!transaction) {
      setError(t("没有找到本次捕获事务，请先点「准备并临时重签」。"));
      return;
    }
    setError('');
    setCaptureBusy('confirm');
    try {
      const check = await invoke<LaunchCheck>('wechat_capture_confirm_launch', { transactionId: transaction });
      if(check.ready_for_preflight) {
        setLaunchDiagnosis(null);
        setNotice(t("已检测到临时微信进程。下一步检查捕获断点。"));
      } else {
        setLaunchDiagnosis(check.diagnosis);
        setNotice(t("还没有检测到稳定的微信进程，原因见下方说明。"));
      }
    } catch(e) {
      setError(captureErrorText(e));
      return capturePrerequisiteFailure(e);
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Brief attach with one administrator prompt; the user still logs out *after* this. */
  const runCapturePreflight = async () => {
    setError('');
    setNotice(t("即将弹出管理员授权对话框：请输入本机密码，密码只交给 macOS，本应用不会读取或保存。"));
    setCaptureBusy('preflight');
    try {
      const result = await invoke<PreflightResult>('wechat_capture_preflight');
      setNotice(tf("断点可用（{0} 处）。现在请在微信里退出账号，然后启动监测。", result.pbkdf_locations));
    } catch(e) {
      setError(captureErrorText(e));
      return capturePrerequisiteFailure(e);
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Long-running: attach LLDB, wait for the next login, validate, restore and import. */
  const startCapture = async () => {
    if(!account) return;
    setError('');
    setNotice(
      captureStatus?.resume_available
        ? t("正在校验上次已捕获的密钥，随后恢复腾讯原版微信并导入。")
        : t("即将弹出管理员授权对话框：请输入本机密码。授权后保持微信窗口打开，等待界面显示「监测已就绪」再登录。"),
    );
    setCaptureBusy('start');
    try {
      const result = await invoke<CaptureImport>('wechat_capture_start', {
        accountId: account.id,
        timeoutSecs: 300,
        ...(recoveringKeys && { targetOnly: true }),
      });
      setCaptureOutcome(result);
      recordCaptureResult({
        matched: result.matched,
        already: result.already,
        unmatched: [],
        accounts: result.accounts,
        message: result.message
      });
      setNotice(result.message);
      await refreshKeys();
      await refreshSavedKeySource();
    } catch(e) {
      setError(captureErrorText(e));
      return capturePrerequisiteFailure(e);
    } finally {
      setCaptureBusy(null);
      await refreshCaptureStatus();
    }
  };

  /** Cancel an in-flight capture or recover from a failed one. */
  const restoreWeChat = async () => {
    setError('');
    setRecoveryError('');
    setLaunchDiagnosis(null);
    setCaptureBusy('restore');
    try {
      const result = await invoke<RestoreResult>('wechat_capture_restore');
      setNotice(
        result.restored
          ? tf("已恢复腾讯原版微信 {0}（{1}），可以重新打开微信。", result.version, result.build)
          : tf("当前微信已经是腾讯原版 {0}，无需恢复。", result.version),
      );
      await refreshCaptureStatus();
    } catch(e) {
      setError(captureErrorText(e)); setRecoveryError(captureErrorText(e));
      await refreshCaptureStatus();
    } finally {
      setCaptureBusy(null);
    }
  };

  /** Resolve an official update without overwriting it with an old backup. */
  const acceptOfficialWeChat = async () => {
    if(captureBusy) return;
    setError(''); setRecoveryError(''); setRecoveryNotice(''); setLaunchDiagnosis(null); setCaptureBusy('accept-official');
    try {
      await invoke('wechat_capture_accept_official');
      setNotice(t("已保留当前官方微信和原备份，可以重新连接。"));
      setRecoveryNotice(t("已校验并保留当前官方微信，原备份已保留。请完成系统检查后继续连接。"));
      await refreshCaptureStatus();
      onWizardStep?.('wizard-permission');
    } catch(e) { setError(captureErrorText(e)); setRecoveryError(captureErrorText(e)); }
    finally { setCaptureBusy(null); }
  };

  const restart = async () => { setError(''); setLaunchDiagnosis(null); await refreshCaptureStatus(); };
  return {
    captureStatus, captureStatusChecking, captureStatusError, recoveryNotice, recoveryError,
    captureBusy, captureOutcome, captureRiskAccepted, setCaptureRiskAccepted, launchDiagnosis,
    refreshCaptureStatus, prepareCapture, openCaptureWeChat, openPrivacySecuritySettings,
    confirmCaptureLaunch, runCapturePreflight, startCapture, restoreWeChat, acceptOfficialWeChat, restart
  };
}

export type WeChatCapture = ReturnType<typeof useWeChatCapture>;
