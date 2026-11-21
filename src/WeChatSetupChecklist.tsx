import { t, tf } from './locale';
import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { DiskAccessCard, type DiskAccessCheck } from './Permission';
import { Icons } from './icons';
import { WizardActionButton } from './WizardActionButton';
import { captureTask, captureAttemptFor, connectionStep, setupChecks, systemEnvironment, type CaptureAttempt, type ConnectionStep, type SetupCapture } from './wechatSetupState';
import type { StepCheck, StepContext, WeChatAction } from './wechatStepGuard';

type Props = {
  accountId: string;
  accounts: { id: string; db_count?: number }[];
  scanLoaded: boolean;
  busy: string | null;
  switching: boolean;
  keyedCount: number;
  keyStatusLoaded: boolean;
  savedKey: { exists: boolean; modified_at: number | null; secure: boolean } | null;
  freshKey: boolean;
  decrypted: boolean;
  capture: SetupCapture | null;
  captureChecking: boolean;
  captureError: string;
  captureBusy: string | null;
  windowsBusy: boolean;
  install: { ready: boolean | null; message: string };
  launchDiagnosis: { amfi_rejected: boolean; quarantined: boolean } | null;
  error: string;
  recoveryError?: string;
  recoveryNotice?: string;
  manual: boolean;
  permissionPhase: boolean;
  finishing: boolean;
  onPermissionNext: () => void;
  onSystemBack: () => void;
  onInstallRecheck: () => void;
  onFixPermissions: () => void;
  onScan: () => void;
  onChooseAccount: (id: string) => void;
  onCaptureRefresh: () => void;
  onPrepare: () => void;
  onOpen: () => void;
  onConfirmLogin: () => void;
  onPreflight: () => void;
  onCapture: () => void;
  onCheckStep: (action: WeChatAction, context: StepContext) => Promise<StepCheck>;
  onRestartConnection: () => void;
  onCheckConnectionResult: () => void;
  onRestore: () => void;
  onAcceptOfficial: () => void;
  onOpenSecurity: () => void;
  onWindowsScan: () => void;
  onImportSaved: () => void;
  onImport: (file: File) => void;
  onDecrypt: () => void;
  onComplete: () => void;
  onBack: () => void;
};

export function WeChatSetupChecklist(p: Props) {
  const [systemInfo, setSystemInfo] = useState<{ platform: string; arch: string; os_version?: string } | null>(null);
  const [systemInfoChecking, setSystemInfoChecking] = useState(false);
  const [systemInfoError, setSystemInfoError] = useState('');
  const refreshSystemInfo = useCallback(async () => {
    setSystemInfoChecking(true);
    try {
      const value = await invoke<{ platform: string; arch: string; os_version?: string }>('app_info');
      setSystemInfo(value);
      setSystemInfoError(value.os_version ? '' : '未能读取系统版本，请更新本地调试客户端后重新检查。');
    } catch {
      setSystemInfo(null);
      setSystemInfoError('未能读取系统版本，请重新检查。');
    } finally {
      setSystemInfoChecking(false);
    }
  }, []);
  useEffect(() => { if (p.permissionPhase) void refreshSystemInfo(); }, [p.permissionPhase, refreshSystemInfo]);
  const [permission, setPermission] = useState<DiskAccessCheck>({ granted: null, platform: null, error: '' });
  const onPermission = useCallback((next: DiskAccessCheck) => setPermission(old =>
    old.granted === next.granted && old.platform === next.platform && old.error === next.error ? old : next), []);
  const [accepted, setAccepted] = useState(false);
  const [openedWechat, setOpenedWechat] = useState(false);
  const [loggedOut, setLoggedOut] = useState(false);
  const [choosingAccount, setChoosingAccount] = useState(false);
  const [confirmedAccount, setConfirmedAccount] = useState<string | null>(null);
  const startedCapture = useRef(false);
  const captureAttempt = useRef<CaptureAttempt | null>(null);
  const [stepIssue, setStepIssue] = useState<{ action: WeChatAction; from: ConnectionStep; check: Exclude<StepCheck, { kind: 'ready' }> } | null>(null);
  const [requestedStep, setRequestedStep] = useState<ConnectionStep | null>(null);
  const [checkingStep, setCheckingStep] = useState(false);
  const [runningStep, setRunningStep] = useState(false);
  const actionLock = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const sectionRef = useRef<HTMLElement | null>(null);
  const actions = useRef(p);
  actions.current = p;
  const attempts = useRef(new Set<string>());
  const hasAccount = p.accounts.some(account => account.id === p.accountId);
  const done = setupChecks({ permission: permission.granted, account: hasAccount && (p.accounts.length <= 1 || confirmedAccount === p.accountId), scanning: p.busy === 'scan' || p.switching,
    keys: p.keyedCount, keyStatusLoaded: p.keyStatusLoaded, savedKey: Boolean(p.savedKey?.exists), freshKey: p.freshKey,
    decrypted: p.decrypted, capture: p.capture, platform: permission.platform });
  const task = captureTask(p.capture);
  captureAttempt.current = captureAttemptFor(captureAttempt.current, p.accountId, p.capture, p.captureBusy);
  const inferredStep = p.launchDiagnosis?.amfi_rejected ? 'recover' : connectionStep(done, p.capture, p.manual, permission.platform, Boolean(captureAttempt.current));
  const current = inferredStep === 'recover' ? 'recover' : stepIssue?.from ?? requestedStep ?? inferredStep;
  const working = Boolean(checkingStep || runningStep || p.busy || p.captureBusy || p.windowsBusy || p.switching || p.finishing);
  const allDone = done.every(Boolean) && current !== 'recover' && !stepIssue;
  const environment = systemEnvironment(p.capture, permission.platform, p.manual || p.keyedCount > 0 || Boolean(p.savedKey?.exists));
  const temporaryWechat = Boolean(p.capture?.pending && p.capture.wechat_ad_hoc && task !== 'recover');
  const installReady = p.install.ready === true || temporaryWechat;
  const dataReady = done[0] && p.scanLoaded && p.accounts.length > 0 && p.busy !== 'scan';
  const needsCapture = !p.manual && p.keyedCount === 0 && !p.savedKey?.exists;
  const systemReady = Boolean(systemInfo?.os_version) && !systemInfoChecking && !systemInfoError && !p.captureError && !p.captureChecking
    && (current === 'recover' || (done[0] && installReady && dataReady && environment.ready));
  const importedSignature = `${p.accountId}:${p.savedKey?.modified_at ?? 'none'}`;

  const runStep = async (action: WeChatAction, run?: () => unknown, checkOnly = false) => {
    if (actionLock.current) return;
    const checkedAccount = p.accountId;
    actionLock.current = true;
    setCheckingStep(true);
    try {
      const check = await actions.current.onCheckStep(action, { platform: permission.platform, captureStarted: Boolean(captureAttempt.current) });
      if (!mounted.current) return;
      if (actions.current.accountId !== checkedAccount) {
        setStepIssue({ action, from: current, check: { kind: 'missing', message: '所选微信账号已变化，请重新确认账号后继续。', target: 'account' } });
        return;
      }
      if (check.kind !== 'ready') {
        setStepIssue({ action, from: current, check });
        return;
      }
      setStepIssue(null);
      setCheckingStep(false);
      if (!checkOnly) {
        setRunningStep(true); setRequestedStep(null);
        const result = await run?.();
        if (mounted.current && result && typeof result === 'object' && 'kind' in result &&
            ['missing', 'check-failed', 'busy'].includes(String(result.kind))) {
          setStepIssue({ action, from: current, check: result as Exclude<StepCheck, { kind: 'ready' }> });
        }
      }
    } catch {
      if (mounted.current) setStepIssue({ action, from: current, check: { kind: 'check-failed', message: '未能完成前置检查，请重新检查。当前进度已保留。' } });
    } finally {
      actionLock.current = false;
      if (mounted.current) { setCheckingStep(false); setRunningStep(false); }
    }
  };

  const recover = async (run: () => unknown) => {
    await run();
    if (mounted.current) { setRequestedStep(null); setStepIssue(null); captureAttempt.current = null; }
  };

  useEffect(() => {
    if (p.permissionPhase || stepIssue || choosingAccount || permission.granted !== true || working || p.capture?.pending || p.capture?.needs_cleanup || current === 'recover') return;
    let key = '';
    let run: (() => void) | undefined;
    if (p.error) {
      return;
    } else if (current === 'verify' && p.savedKey?.exists && p.savedKey.secure) {
      key = `import:${importedSignature}`; run = () => { void runStep('import-saved', actions.current.onImportSaved); };
    } else if (current === 'read' && !p.decrypted) {
      key = `decrypt:${p.accountId}:${p.keyedCount}:${p.savedKey?.modified_at}`; run = () => { void runStep('decrypt', actions.current.onDecrypt); };
    }
    if (key && run && !attempts.current.has(key)) { attempts.current.add(key); run(); }
  }, [p.permissionPhase, stepIssue, choosingAccount, permission.granted, working, p.error, p.capture, current, p.savedKey, p.decrypted, importedSignature, p.accountId, p.keyedCount, hasAccount]);

  useEffect(() => { setAccepted(false); setOpenedWechat(false); setLoggedOut(false); startedCapture.current = false; setStepIssue(null); setRequestedStep(null); }, [p.accountId]);
  useEffect(() => { if (p.permissionPhase) { setStepIssue(null); setRequestedStep(null); } }, [p.permissionPhase]);
  useEffect(() => { if (inferredStep === 'recover') { setStepIssue(null); setRequestedStep(null); } }, [inferredStep]);
  useEffect(() => { if (p.capture?.pending) startedCapture.current = true; }, [p.capture?.pending]);
  useEffect(() => { setOpenedWechat(false); setLoggedOut(false); }, [p.capture?.stage]);
  useEffect(() => {
    if (!p.permissionPhase) sectionRef.current?.querySelector('[aria-label="微信操作步骤"] > .current')?.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [p.permissionPhase, current]);

  const fileInput = <label className="wxv-btn wx-setup-file">{t("选择密钥文件")}<input type="file" accept=".json,application/json,text/plain" disabled={working || !hasAccount} onChange={event => { const file = event.target.files?.[0]; if (file) void runStep('import-file', () => p.onImport(file)); event.target.value = ''; }} /></label>;
  const stepError = current === 'recover' ? p.recoveryError : p.error;
  const errorDetails = stepError && <details className="wx-setup-details"><summary>{t("查看错误详情")}</summary><p>{stepError}</p></details>;
  const accountChoices = <div className="wx-setup-accounts"><p>{t("选择要连接的微信账号：")}</p>{p.accounts.map(account => <label key={account.id}><input type="radio" name="setup-account" checked={p.accountId === account.id} disabled={working || Boolean(p.capture?.pending)} onChange={() => { setChoosingAccount(false); p.onChooseAccount(account.id); }} />{account.id}</label>)}<button className="wxv-btn primary" disabled={!hasAccount || working} onClick={() => { setConfirmedAccount(p.accountId); setChoosingAccount(false); }}>{t("使用此账号")}</button></div>;
  const failure = (message: string) => stepError && !working ? <p className="wx-setup-error" role="alert">{message}</p> : null;

  const connectionBody = (step: ConnectionStep) => {
    if (step === 'account') return p.busy === 'scan' || p.switching ? <p role="status">{t("正在查找微信账号，请稍候。")}</p>
      : p.capture?.pending && !hasAccount ? <p role="alert">{t("未找到本次连接的账号数据。请先取消连接并恢复微信，再重新确认账号。")}</p>
      : p.accounts.length > 0 ? accountChoices
        : <><p>{p.scanLoaded ? t("没有找到微信账号。请打开微信并登录一次，再重新查找。") : t("正在查找本机微信账号。")}</p><WizardActionButton className="wxv-btn primary" loading={p.busy === 'scan'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在查找账号…")} disabled={working} onAction={p.onScan}>{p.scanLoaded ? t("重新查找账号") : t("查找微信账号")}</WizardActionButton></>;
    if (step === 'recover') return <>
      {p.capture?.recovery_error_code === 'external_install_conflict' && p.capture.wechat_official ? <>
        <p>{t("微信已更新。点击「使用当前微信继续」即可，无需重启微信；原备份会保留。")}</p>
        <p>{t("程序会校验当前微信，处理上次连接留下的记录，然后回到连接准备。")}</p>
        {failure(t("校验未完成，请查看错误详情，处理后重新点击「使用当前微信继续」。"))}
        <WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'accept-official'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在校验当前微信…")} disabled={working} onAction={() => recover(p.onAcceptOfficial)}>{t("使用当前微信继续")}</WizardActionButton>
      </> : <>
        <p>{t("上次连接没有正常结束。请点击「恢复微信」，完成后再重新连接，无需手动重启。")}</p>
        {failure(t("恢复未完成，请查看错误详情，处理后点击「重试恢复微信」。"))}
        <WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'restore'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在恢复微信…")} disabled={working} onAction={() => recover(p.onRestore)}>{p.recoveryError ? t("重试恢复微信") : t("恢复微信")}</WizardActionButton>
      </>}
    </>;
    if (step === 'file') return <><p>{t("选择已准备好的微信密钥文件。")}</p>{failure(t("文件未能导入，请确认文件属于当前账号，再重新选择。"))}{fileInput}</>;
    if (step === 'windows') return <><p>{t("请保持微信已登录，再点击「连接微信」。")}</p>{failure(t("连接未完成。请确认 AIMyChats 与微信使用同一 Windows 用户，必要时以管理员身份重新打开 AIMyChats 后重试。"))}<WizardActionButton className="wxv-btn primary" loading={Boolean(p.windowsBusy)} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在连接…")} disabled={working || !hasAccount} onAction={() => runStep('windows', p.onWindowsScan)}>{p.error ? t("重试连接微信") : t("连接微信")}</WizardActionButton></>;
    if (step === 'prepare') {
      if (task === 'checking' || p.captureError) return <><p>{t("未能取得微信连接状态，请重新检查。")}</p><WizardActionButton className="wxv-btn primary" loading={p.captureChecking} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在检查…")} disabled={working} onAction={p.onCaptureRefresh}>{t("重新检查连接状态")}</WizardActionButton></>;
      if (!environment.ready) return <><p>{t("系统检查条件已发生变化，请返回检查后继续。")}</p><button className="wxv-btn primary" disabled={working} onClick={p.onSystemBack}>{t("返回系统检查")}</button></>;
      return <>
        <p>{t("微信会退出。程序将备份微信并临时调整签名，连接结束后恢复；期间需要重新登录。")}</p>
        <label className="wx-setup-confirm"><input type="checkbox" checked={accepted} disabled={working} onChange={event => setAccepted(event.target.checked)} />{t("我已保存当前工作，同意上述操作，并确认读取的是本人微信数据")}</label>
        {failure(t("准备未完成，请点击「重新准备微信」。仍失败时，请查看错误详情并按提示处理。"))}
        <WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'prepare'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在备份并准备微信…")} disabled={!accepted || working} onAction={() => runStep('prepare', () => { startedCapture.current = true; return p.onPrepare(); })}>{p.error ? t("重新准备微信") : t("准备微信")}</WizardActionButton>
      </>;
    }
    if (step === 'login') return <>
      <p>{t("打开微信，登录要连接的账号。登录完成后，点击「我已登录，继续」。")}</p>
      {p.launchDiagnosis && <div className="wx-setup-problem"><p>{t("微信被系统拦下。请到「隐私与安全性 → 安全性」中允许打开，再重试。")}</p><button className="wxv-btn" disabled={working} onClick={p.onOpenSecurity}>{t("打开隐私与安全性")}</button></div>}
      {failure(t("尚未确认微信已登录。请完成登录后重新确认；微信未打开时，可点击「重新打开微信」。"))}
      <div className="wx-setup-actions"><WizardActionButton className="wxv-btn" loading={p.captureBusy === 'open'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在打开微信…")} disabled={working} onAction={() => runStep('open', () => { setOpenedWechat(true); return p.onOpen(); })}>{openedWechat || p.error ? t("重新打开微信") : t("打开微信")}</WizardActionButton>
        <WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'confirm'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在确认登录…")} disabled={working} onAction={() => runStep('confirm', p.onConfirmLogin)}>{t("我已登录，继续")}</WizardActionButton></div>
    </>;
    if (step === 'authorize') return <><p>{t("保持微信已登录，点击下方按钮，并在系统弹窗中完成授权。")}</p>{failure(t("授权检查未完成，请重新点击按钮，并按系统弹窗提示操作。"))}<WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'preflight'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("请在系统弹窗中授权…")} disabled={working} onAction={() => runStep('preflight', p.onPreflight)}>{p.error ? t("重新授权并检查") : t("授权并检查")}</WizardActionButton></>;
    if (step === 'logout') return <><p>{t("请在微信中退出当前账号，保留登录窗口，暂时不要重新登录。")}</p><label className="wx-setup-confirm"><input type="checkbox" checked={loggedOut} disabled={working} onChange={event => setLoggedOut(event.target.checked)} />{t("我已退出微信账号，并保留登录窗口")}</label>{failure(t("连接准备未完成，请确认微信窗口仍打开，再点击「重试连接准备」。"))}<WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'start'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在准备连接…")} disabled={!loggedOut || working} onAction={() => runStep('capture', p.onCapture)}>{p.error ? t("重试连接准备") : t("已退出，继续")}</WizardActionButton></>;
    if (step === 'relogin') {
      if (task === 'resume') return <><p>{t("上次连接已取得结果，点击继续即可。")}</p>{failure(t("继续连接未完成，请重试；仍失败时，请先恢复微信。"))}<WizardActionButton className="wxv-btn primary" loading={p.captureBusy === 'start'} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在完成连接…")} disabled={working} onAction={() => runStep('capture', p.onCapture)}>{t("继续完成连接")}</WizardActionButton></>;
      if (p.error && !working) return p.capture?.pending || p.capture?.needs_cleanup
        ? <><p role="alert">{t("连接已中断，请先恢复微信，再重新连接。")}</p><WizardActionButton className="wxv-btn primary" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在恢复微信…")} onAction={() => recover(p.onRestore)}>{t("恢复微信后重试")}</WizardActionButton></>
        : <><p role="alert">{t("本次连接未完成。")}{p.capture?.wechat_official ? t("微信已恢复为官方版本，无需再次恢复。") : ''}{t("请查看错误详情，处理后点击「重新连接」。")}</p><WizardActionButton className="wxv-btn primary" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在检查连接状态…")} onAction={() => runStep('restart', () => { captureAttempt.current = null; setAccepted(false); setOpenedWechat(false); setLoggedOut(false); return p.onRestartConnection(); })}>{t("重新连接")}</WizardActionButton></>;
      if (!p.capture?.pending) return working
        ? <p role="status">{t("正在处理连接结果，请稍候，无需重新准备微信。")}</p>
        : <><p role="alert">{t("尚未确认连接结果，请点击「重新检查连接结果」。检查通过后会继续校验。")}</p><WizardActionButton className="wxv-btn primary" loading={p.captureChecking} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在检查连接结果…")} onAction={() => runStep('result', p.onCheckConnectionResult)}>{t("重新检查连接结果")}</WizardActionButton></>;
      return <p role="status">{p.capture?.capture_phase === 'waiting_authorization' ? t("请在系统弹窗中授权，暂时不要登录微信。")
        : p.capture?.capture_phase === 'monitoring' && p.capture.monitor_ready ? t("现在请重新登录同一个微信账号，登录后会自动继续。")
          : p.capture?.capture_phase === 'captured' || p.capture?.capture_phase === 'validating' ? t("已收到登录结果，正在校验，请稍候。")
            : p.capture?.capture_phase === 'restoring' ? t("正在恢复微信，请稍候。") : t("正在准备连接，请等待“现在请重新登录”的提示。")}</p>;
    }
    if (step === 'verify') {
      if (p.savedKey?.exists && !p.savedKey.secure) return <><p>{t("连接文件的访问权限需要修复，点击下方按钮后继续。")}</p><WizardActionButton className="wxv-btn primary" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在修复文件权限…")} disabled={working} onAction={p.onFixPermissions}>{t("修复文件权限")}</WizardActionButton></>;
      return <><p role={working ? 'status' : undefined}>{p.busy === 'import' ? t("正在校验微信连接，请稍候。") : p.error ? t("校验未通过。请重新校验；若文件属于其他账号，请重新选择文件。") : t("即将校验微信连接，请稍候。")}</p>{!working && <div className="wx-setup-actions">{p.savedKey?.exists && <WizardActionButton className="wxv-btn primary" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在校验…")} onAction={() => runStep('import-saved', p.onImportSaved)}>{t("重新校验")}</WizardActionButton>}{fileInput}</div>}</>;
    }
    return <><p role={working ? 'status' : undefined}>{p.busy === 'decrypt' || p.busy === 'messages' ? t("正在读取聊天记录，请稍候。") : p.error ? t("读取未完成，请点击「重新读取」。仍失败时，请查看错误详情。") : t("即将读取聊天记录，请稍候。")}</p>{!working && <WizardActionButton className="wxv-btn primary" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在读取…")} onAction={() => runStep('decrypt', p.onDecrypt)}>{t("重新读取")}</WizardActionButton>}</>;
  };

  const reuse = !p.manual && permission.platform === 'macos' && done[2] && !startedCapture.current
    && ['verify', 'read', 'complete'].includes(current);
  const connectionRows: Array<[ConnectionStep, string]> = [
    ...(p.accounts.length !== 1 || !done[1] ? [['account', '确认微信账号'] as [ConnectionStep, string]] : []),
    ...(current === 'recover' ? [['recover', '恢复微信'] as [ConnectionStep, string]] : []),
    ...(p.manual && !p.capture?.pending ? [['file', '导入微信连接文件'] as [ConnectionStep, string]]
      : permission.platform === 'windows' ? [['windows', '连接微信'] as [ConnectionStep, string]]
        : [['prepare', '准备微信'], ['login', '登录微信'], ['authorize', '授权连接'], ['logout', '退出微信账号'], ['relogin', '重新登录微信']] as Array<[ConnectionStep, string]>),
    ['verify', '校验连接'], ['read', '读取聊天记录'],
  ];
  const currentIndex = current === 'complete' ? connectionRows.length : connectionRows.findIndex(([id]) => id === current);
  const skipCaptureChecks = !needsCapture || permission.platform === 'windows';
  const databaseCount = p.accounts.reduce((total, account) => total + (account.db_count ?? 0), 0);
  const arm64 = systemInfo?.arch === 'aarch64' || systemInfo?.arch === 'arm64';
  const archLabel = arm64 ? systemInfo?.platform === 'macos' ? 'Apple Silicon（ARM64）' : 'ARM64' : systemInfo?.arch ?? '';
  const captureCheckPending = !p.captureError && (!p.capture || p.captureChecking);
  const systemRows = [
    { id: 'os', title: '系统版本', detail: systemInfo?.os_version ?? '', passed: false, information: Boolean(systemInfo?.os_version), checking: systemInfoChecking, skipped: false, status: systemInfo?.os_version ? '兼容性待验证' : '' },
    { id: 'arch', title: '芯片架构', detail: archLabel, passed: permission.platform === 'windows' || Boolean(p.capture?.platform_supported), checking: (!systemInfoError && !systemInfo) || (permission.platform !== 'windows' && captureCheckPending), skipped: skipCaptureChecks },
    { id: 'install', title: '微信安装', detail: p.capture?.wechat_version ? `微信 ${p.capture.wechat_version}${p.capture.wechat_build ? `（${p.capture.wechat_build}）` : ''}${temporaryWechat ? ' · 本次连接的临时版本' : p.capture.wechat_official ? ' · 官方签名' : ''}` : '', passed: installReady && !temporaryWechat, information: temporaryWechat, checking: p.install.ready === null, skipped: false },
    { id: 'permission', title: '完全磁盘访问权限', passed: done[0], checking: permission.granted === null && !permission.error, skipped: false },
    { id: 'data', title: '微信数据目录', detail: dataReady ? `已找到 ${p.accounts.length} 个账号目录${databaseCount ? ` · ${databaseCount} 个数据库` : ''}` : '', passed: dataReady, checking: p.busy === 'scan', skipped: false, status: !done[0] ? '等待授权' : '' },
    { id: 'tools', title: '系统工具', detail: p.capture?.lldb_available && !skipCaptureChecks ? 'LLDB 与 Python 运行检查通过' : '', passed: Boolean(p.capture?.lldb_available) && !p.captureError, checking: captureCheckPending, skipped: skipCaptureChecks },
    { id: 'write', title: '安装目录读写权限', passed: Boolean(p.capture?.user_can_write) && !p.captureError, checking: captureCheckPending, skipped: skipCaptureChecks },
    { id: 'readonly', title: '数据访问方式', detail: '只读检查，不修改原始微信数据库。', passed: false, information: true, checking: false, skipped: false, status: '只读' },
  ];

  const targetLabel = (target: ConnectionStep | 'system') => ({
    system: '系统检查', account: '确认微信账号', prepare: '准备微信', login: '登录微信', authorize: '授权连接',
    logout: '退出微信账号', relogin: '检查连接结果', recover: '恢复微信', file: '选择连接文件', windows: '连接微信',
    verify: '校验连接', read: '读取聊天记录', complete: '连接完成',
  })[target];
  const stepProblem = stepIssue && <div className="wx-setup-problem">
    <p role={stepIssue.check.kind === 'busy' ? 'status' : 'alert'}>{t(stepIssue.check.message)}</p>
    <div className="wx-setup-actions">
      {stepIssue.check.kind !== 'missing' && <WizardActionButton className="wxv-btn primary" loading={checkingStep} busyLabel={t("正在检查前置条件…")} disabled={working} onAction={() => runStep(stepIssue.action, undefined, true)}>{t("重新检查")}</WizardActionButton>}
      {stepIssue.check.target && <button className={`wxv-btn ${stepIssue.check.kind === 'missing' ? 'primary' : ''}`} disabled={working} onClick={() => {
        const target = stepIssue.check.target!;
        setStepIssue(null);
        if (target === 'system') p.onSystemBack();
        else {
          if (target === 'prepare' || target === 'file') { captureAttempt.current = null; setAccepted(false); setLoggedOut(false); }
          setRequestedStep(target);
        }
      }}>{t("返回")}{targetLabel(stepIssue.check.target)}</button>}
    </div>
  </div>;

  return <section ref={sectionRef} className="wx-setup" aria-label={p.permissionPhase ? t("系统检查") : t("连接微信")}>
    <header><h2>{p.permissionPhase ? t("系统检查") : t("连接微信")}</h2><p>{p.permissionPhase ? t("检查完成后，再开始连接微信。") : t("按顺序操作，完成当前一步后会显示下一步。")}</p></header>
    {p.recoveryNotice && <p className="wx-setup-passed" role="status">{p.recoveryNotice}</p>}
    {p.permissionPhase && stepProblem}
    <ol className="wx-setup-list" hidden={!p.permissionPhase}>
      {systemRows.map(row => {
        const completed = row.skipped || ((row.information || row.passed) && !row.checking);
        return <li key={row.id} className={`wx-setup-item ${completed ? 'done' : 'current'}`}>
        <div className="wx-setup-row"><span className="wx-setup-indicator" aria-hidden="true">{completed ? Icons.check : Icons.clock}</span><span className="wx-setup-row-copy"><strong>{t(row.title)}</strong>{row.detail && <small>{t(row.detail)}</small>}{row.status && <small>{t(row.status)}</small>}</span><span className="wx-setup-status">{row.checking && !completed && <svg className="wizard-button-spinner" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 2a6 6 0 0 1 6 6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" /></svg>}{completed && <span className="wizard-status-check" aria-hidden="true">{Icons.check}</span>}{completed ? t("已完成") : t("未完成")}</span></div>
        <div className="wx-setup-body" hidden={row.passed || row.skipped || row.information}>
          {row.id === 'os' && systemInfoError && <><p role="alert">{systemInfoError}</p><button className="wxv-btn" disabled={systemInfoChecking} onClick={() => void refreshSystemInfo()}>{t("重新读取系统版本")}</button></>}
          {row.id === 'arch' && !row.checking && <><p>{t("当前架构暂不支持自动取钥。请使用 Apple Silicon 版本的应用，或返回接入方式选择导入文件。")}</p><button className="wxv-btn" onClick={p.onBack}>{t("返回接入方式")}</button></>}
          {row.id === 'install' && !row.checking && <><p>{p.install.message || "请安装官方微信后重新检查。"}</p><WizardActionButton className="wxv-btn" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在检查微信…")} onAction={p.onInstallRecheck}>{t("重新检查微信")}</WizardActionButton></>}
          {row.id === 'permission' && <DiskAccessCard compact checkOnEntry={p.permissionPhase} onCheck={onPermission} />}
          {row.id === 'data' && done[0] && !row.checking && <><p>{t("未找到可读取的微信数据目录。请先打开微信并登录一次，再重新检查。")}</p><WizardActionButton className="wxv-btn" busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在查找数据目录…")} disabled={working} onAction={p.onScan}>{t("重新查找数据目录")}</WizardActionButton></>}
          {(row.id === 'tools' || row.id === 'write') && <>{!row.checking && <p role="alert">{p.captureError ? t("检查未完成，请重试。") : row.id === 'tools' ? t("请安装或修复 Xcode Command Line Tools 后，重新检查系统工具。") : t("请确认当前用户对微信及其安装目录有读写权限，再重新检查。")}</p>}<WizardActionButton className="wxv-btn" loading={p.captureChecking} busyLabel={checkingStep ? t("正在检查前置条件…") : t("正在检查…")} onAction={p.onCaptureRefresh}>{t("重新检查")}{row.id === 'tools' ? t("系统工具") : t("目录权限")}</WizardActionButton></>}
        </div>
      </li>; })}
    </ol>
    {p.permissionPhase && current === 'recover' && <div className="wx-setup-problem"><p>{t("请先点「下一步：处理微信恢复」，处理上次连接留下的记录。这项操作不读取聊天记录；完成后再回来检查读取权限。")}</p></div>}
    {!p.permissionPhase && <>
      {!done[0] && <div className="wx-setup-problem"><p>{t("读取权限尚未通过，请返回系统检查。")}</p><button className="wxv-btn" disabled={working || Boolean(p.capture?.pending)} onClick={p.onSystemBack}>{t("返回系统检查")}</button></div>}
      {reuse && <p className="wxv-muted" role="status">{done[3] ? t("已复用当前账号保存的连接结果，无需重复取钥。") : t("检测到已保存的连接文件，将先校验是否适用于当前账号。")}</p>}
      {current === 'complete' && stepProblem}
      <ol className="wx-setup-list" aria-label={t("微信操作步骤")}>
        {connectionRows.map(([id, title], index) => {
          const missingIndex = stepIssue?.check.kind === 'missing' ? connectionRows.findIndex(([row]) => row === stepIssue.check.target) : -1;
          const reused = reuse && ['prepare', 'login', 'authorize', 'logout', 'relogin'].includes(id);
          const passed = (current === 'recover' ? id === 'account' && done[1] : current === 'complete' || index < currentIndex) && (missingIndex < 0 || index < missingIndex);
          const completed = reused ? done[3] && passed : passed;
          const expanded = id === current;
          return <li key={id} className={`wx-setup-item ${completed ? 'done' : expanded ? 'current' : 'waiting'}`}>
            <div className="wx-setup-row" aria-current={expanded ? 'step' : undefined}><span className="wx-setup-indicator" aria-hidden="true">{completed ? Icons.check : index + 1}</span><strong>{t(title)}</strong><span className="wx-setup-status">{expanded && working && <svg className="wizard-button-spinner" viewBox="0 0 16 16" fill="none" aria-hidden="true"><path d="M8 2a6 6 0 0 1 6 6" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" /></svg>}{completed && <span className="wizard-status-check" aria-hidden="true">{Icons.check}</span>}{completed ? t("已完成") : t("未完成")}</span></div>
            {expanded && <div className="wx-setup-body">{stepIssue && current !== 'recover' ? stepProblem : connectionBody(id)}{!stepIssue && errorDetails}
              {p.capture?.pending && id !== 'recover' && <button className="wx-setup-cancel" disabled={Boolean(p.captureBusy)} onClick={() => { void recover(p.onRestore); }}>{t("取消连接并恢复微信")}</button>}
            </div>}
          </li>;
        })}
      </ol>
      {p.accounts.length > 1 && current !== 'account' && <button className="wx-setup-switch" disabled={working || Boolean(p.capture?.pending)} onClick={() => setChoosingAccount(value => !value)}>{choosingAccount ? t("取消切换账号") : t("切换微信账号")}</button>}
      {choosingAccount && accountChoices}
      {allDone && <p className="wx-setup-passed" role="status">{t("微信已连接，聊天记录已就绪。")}</p>}
    </>}
    <footer><button className="wxv-btn" disabled={working || Boolean(p.capture?.pending)} onClick={p.onBack}>{t("上一步")}</button><WizardActionButton className="wxv-btn primary" loading={checkingStep || p.finishing} busyLabel={p.finishing ? t("正在保存…") : t("正在检查前置条件…")} disabled={working || choosingAccount || Boolean(stepIssue) || (p.permissionPhase ? !systemReady : !allDone)} onAction={() => runStep(p.permissionPhase ? 'connect' : 'complete', p.permissionPhase ? p.onPermissionNext : p.onComplete)}>{p.permissionPhase ? current === 'recover' ? t("下一步：处理微信恢复") : t("下一步：连接微信") : t("下一步：选择会话")}</WizardActionButton></footer>
  </section>;
}
