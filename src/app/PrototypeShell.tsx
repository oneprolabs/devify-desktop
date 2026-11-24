import { Fragment, useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from '../shared/api/desktopInvoke';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import WeChatLocal from '../features/chat/local-reader/WeChatLocal';
import { WhatsAppWizard } from '../features/connectors/wizard/WhatsAppWizard';
import { WeChatWizardComplete } from '../features/connectors/wizard/WeChatWizardComplete';
import { AgentHub } from '../features/agents/AgentHub';
import { About, UpdatePanel } from '../features/settings/About';
import { checkForUpdates, useUpdater } from '../features/updates/updater';
import { HistoryPage } from '../features/chat/summary/History';
import { DevifySettings, WhitelistSettings } from '../features/settings/Settings';
import type { AppSettings } from '../features/settings/settingsModel';
import { BackupSchedule } from '../features/backup/BackupSchedule';
import { AnalysisSettings } from '../features/analysis/AnalysisSettings';
import { LanguageSettings } from '../features/settings/LanguageSettings';
import { ConnectionSettingsDisclosure, NetworkConnectionControl, NetworkSettings } from '../features/settings/NetworkSettings';
import '../features/settings/settings-pages.css';
import { Icons } from '../shared/ui/icons';
import { NavigationRail, type Section } from './NavigationRail';
import { detectTitle, useConnectorDetect } from '../features/connectors/connectorDetect';
import { Connectors } from '../features/connectors/Connectors';
import wechatIcon from '../assets/connectors/wechat.svg';
import { CloudAnalysisPage } from '../features/analysis/CloudAnalysis';
import { TopicsPage } from '../features/analysis/Topics';
import { AskPage } from '../features/analysis/Ask';
import { Tasks } from '../features/tasks/Tasks';
import { ChatEntry } from '../features/chat/ChatLanding';
import { ChatWorkbench } from '../features/chat/ChatWorkbench';
import { LoginGate } from '../features/auth/LoginGate';
import { Feedback, FeedbackSettings } from '../features/feedback/Feedback';
import { requestFeedback } from '../shared/api/desktopInvoke';
import { registerDevifyDevice } from '../shared/api/devifyDevice';
import { resetTaskRuntime, useTaskRuntime } from '../features/tasks/taskRuntime';
import { clearTaskSnapshot } from '../features/tasks/taskSnapshot';
import { clearPageSnapshots } from '../shared/state/pageSnapshots';
import { granted, refreshEntitlements, useInternalAccount } from '../features/auth/entitlements';
import { applyLanguage, useUiLanguage, t, tf, ctf } from '../shared/i18n/locale';
import { errorText } from '../shared/i18n/errorText';
import { shouldApplyAuthExpired } from '../features/auth/authEvents';
import './styles/prototype-shell.css';
import './styles/reference-theme.css';
import './styles/desktop-visual-refresh.css';
import '../features/chat/chat-conversation.css';
import '../features/analysis/analysis-workspace.css';
import './styles/navigation-rail.css';
import '../features/connectors/connectors.css';
import '../features/connectors/wizard/wechat-wizard.css';
import brandMark from '../assets/app-mark-transparent.png';
import { DEVELOPMENT_APP } from '../shared/api/buildFlavor';
import { DevelopmentEnvironment } from '../features/settings/DevelopmentEnvironment';

type SettingsPage = 'connectors' | 'imports' | 'backup' | 'analysis' | 'language' | 'account' | 'network' | 'history' | 'agent' | 'feedback' | 'about';

function handleWindowDrag(event: MouseEvent<HTMLElement>) {
  if (!isTauri() || event.button !== 0 || event.clientY - event.currentTarget.getBoundingClientRect().top >= 72) return;
  if ((event.target as Element).closest('button, input, a, select, textarea, [contenteditable], [role="tab"], [role="separator"], [role="dialog"], [role="menuitem"]')) return;
  if (event.currentTarget.querySelector('[aria-modal="true"]')) return;
  if (event.detail === 2) {
    event.preventDefault();
    void getCurrentWindow().toggleMaximize();
  } else if (event.detail === 1) {
    event.preventDefault();
    void getCurrentWindow().startDragging();
  }
}

const settingsPages: { id: SettingsPage; label: string; group: 'main' | 'local' }[] = [
  // The first six follow protype.html (连接器 / 导入管理 / 备份周期 / 智能分析 / 语言 / 账户);
  // the rest are pages this build has on top of the prototype.
  { id: 'connectors', label: '连接器', group: 'main' },
  { id: 'imports', label: '导入管理', group: 'main' },
  { id: 'backup', label: '备份周期', group: 'main' },
  { id: 'analysis', label: '智能分析', group: 'main' },
  { id: 'language', label: '语言', group: 'main' },
  { id: 'account', label: '账户', group: 'main' },
  { id: 'network', label: '网络连接', group: 'main' },
  // Local summaries are saved on this machine only; the prototype has no page for
  // them, but without this entry the records are unreachable from settings.
  { id: 'history', label: '总结记录', group: 'local' },
  { id: 'agent', label: '智能体集成', group: 'local' },
  { id: 'feedback', label: '问题反馈', group: 'local' },
  { id: 'about', label: '关于', group: 'local' },
];
const settingsIcons: Record<SettingsPage, ReactNode> = {
  connectors: Icons.key, imports: Icons.download, backup: Icons.clock,
  analysis: Icons.agent, language: Icons.list, account: Icons.allow, network: Icons.send,
  history: Icons.history, agent: Icons.agent, feedback: Icons.chat, about: Icons.info,
};

export default function PrototypeShell() {
  useUiLanguage();
  useEffect(() => {
    let active = true;
    void invoke<AppSettings>('app_settings_get').then(value => { if (active) applyLanguage(value.language); }).catch(() => {});
    return () => { active = false; };
  }, []);
  const [section, setSection] = useState<Section>('chat');
  const [feedbackRequest, setFeedbackRequest] = useState<{ category: string; page: string; relatedBatchIds: number[] } | null>(null);
  useEffect(() => {
    const open = (event: Event) => {
      const detail = (event as CustomEvent).detail;
      setFeedbackRequest({ category: detail?.category || 'other', page: detail?.page || 'app', relatedBatchIds: detail?.relatedBatchIds || [] });
    };
    window.addEventListener('app:feedback', open);
    return () => window.removeEventListener('app:feedback', open);
  }, []);
  const update = useUpdater();
  const [updateDialogOpen, setUpdateDialogOpen] = useState(false);
  useEffect(() => {
    if (!updateDialogOpen) return;
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === 'Escape') setUpdateDialogOpen(false); };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [updateDialogOpen]);
  useEffect(() => {
    if (DEVELOPMENT_APP) return;
    const refresh = () => { void checkForUpdates(true); };
    refresh();
    window.addEventListener('focus', refresh);
    window.addEventListener('online', refresh);
    const timer = window.setInterval(refresh, 5 * 60 * 1000);
    return () => {
      window.removeEventListener('focus', refresh);
      window.removeEventListener('online', refresh);
      window.clearInterval(timer);
    };
  }, []);
  const [eventFocusId, setEventFocusId] = useState<number | null>(null);
  const [chatReady, setChatReady] = useState(false);
  const [chatSource, setChatSource] = useState('wechat');
  const [chatKey, setChatKey] = useState(0);
  const [firstBackupStep, setFirstBackupStep] = useState<'none' | 'select' | 'schedule' | 'confirm'>('none');
  const [startFirstBackup, setStartFirstBackup] = useState(false);
  const [backupPreview, setBackupPreview] = useState<AppSettings | null>(null);
  useEffect(() => {
    if (firstBackupStep !== 'confirm') return;
    let active = true;
    setBackupPreview(null);
    void invoke<AppSettings>('app_settings_get').then(value => { if (active) setBackupPreview(value); }).catch(() => {});
    return () => { active = false; };
  }, [firstBackupStep]);
  const [connectionFinishError, setConnectionFinishError] = useState('');
  const [connectionFinishing, setConnectionFinishing] = useState(false);
  const [startTaskCreate, setStartTaskCreate] = useState(false);
  const [authState, setAuthState] = useState<'checking' | 'authed' | 'anonymous' | 'unavailable'>('checking');
  const authEpoch = useRef(0);
  const authCheckPending = useRef(false);
  const [authChecking, setAuthChecking] = useState(false);
  const [authNotice, setAuthNotice] = useState('');
  const [authError, setAuthError] = useState('');
  const [connectionSaving, setConnectionSaving] = useState(false);
  const [connectionSettingsOpen, setConnectionSettingsOpen] = useState(false);
  const [currentUser, setCurrentUser] = useState('');
  const [currentUserId, setCurrentUserId] = useState<number | null>(null);
  const [authVersion, setAuthVersion] = useState(0);
  const [connectionState, setConnectionState] = useState<'connected' | 'unavailable'>('unavailable');
  const authMounted = useRef(true);
  useEffect(() => { authMounted.current = true; return () => { authMounted.current = false; }; }, []);
  useEffect(() => {
    let active = true;
    setCurrentUser('');
    setCurrentUserId(null);
    if (authState === 'authed') void invoke<{ id?: number; display_name?: string; username?: string }>('devify_current_user').then(user => { if (active) { setCurrentUser(user.display_name || user.username || ''); setCurrentUserId(Number.isSafeInteger(user.id) && (user.id ?? 0) > 0 ? user.id! : null); } }).catch(() => { if (active) setCurrentUser(''); });
    return () => { active = false; };
  }, [authState, authVersion]);
  useEffect(() => { resetTaskRuntime(authState === 'authed'); }, [authState, authVersion]);
  const handleAuthenticated = useCallback(() => { resetTaskRuntime(true); clearTaskSnapshot(); clearPageSnapshots(); authEpoch.current += 1; setCurrentUserId(null); setCurrentUser(''); setAuthVersion(version => version + 1); setAuthState('authed'); setConnectionState('connected'); setAuthNotice(''); setAuthError(''); setChatReady(false); setSection('chat'); setEventFocusId(null); setStartTaskCreate(false); void registerDevifyDevice(); }, []);
  /**
   * The backend caps every Devify request with a timeout, so this resolves even when the
   * server is unreachable. `unavailable` is a separate state from "logged out": it must not
   * drop the stored session, and the user gets an explicit "retry later" instead of a spinner.
   * Every result is dropped once the epoch moved on, so a slow reply from a previous login
   * cannot overwrite the current session state.
   */
  const checkAuth = useCallback(() => {
    if (authCheckPending.current) return;
    authCheckPending.current = true;
    const requestEpoch = authEpoch.current;
    // A retry keeps the current explanation visible. Replacing the whole gate
    // with the initial loading screen made every background retry look like a redirect.
    setAuthChecking(true);
    void invoke<{ authenticated: boolean; unavailable?: boolean; error?: string }>('devify_auth_status')
      .then(status => {
        if (!authMounted.current || authEpoch.current !== requestEpoch) return;
        if (status.unavailable) {
          setAuthError(status.error || t("服务端暂不可用，请稍后重试。"));
          setAuthState('unavailable');
          return;
        }
        setAuthState(status.authenticated ? 'authed' : 'anonymous');
        if (status.authenticated) setConnectionState('connected');
        if (status.authenticated) {
          void registerDevifyDevice();
        }
      })
      .catch(() => { if (authMounted.current && authEpoch.current === requestEpoch) { setAuthError(t("暂时无法检查服务连接，请稍后重试。")); setAuthState('unavailable'); } })
      .finally(() => {
        authCheckPending.current = false;
        if (authMounted.current) setAuthChecking(false);
      });
  }, []);
  useEffect(() => {
    let active = true;
    let cleanExpired: (() => void) | undefined;
    checkAuth();
    // The backend drops stale credentials and emits this when the server rejects
    // the stored session (expired, revoked or deleted account), so the app falls
    // back to the login gate instead of showing every Devify page as broken.
    // A queued expiry event may belong to a previous login. Recheck the current
    // credentials and discard the callback if another login finishes meanwhile.
    void listen<{ message?: string }>('devify-auth-expired', event => {
      const eventEpoch = authEpoch.current;
      void shouldApplyAuthExpired(
        () => invoke<{ authenticated: boolean }>('devify_auth_status'),
        () => active && authEpoch.current === eventEpoch,
      ).then(apply => {
        if (!apply || !active || authEpoch.current !== eventEpoch) return;
        clearTaskSnapshot();
        clearPageSnapshots();
        authEpoch.current += 1;
        setAuthNotice(event.payload?.message ?? t("登录状态已失效，请重新登录。"));
        setAuthState('anonymous');
      });
    }).then(unlisten => { if (active) cleanExpired = unlisten; else unlisten(); }).catch(() => {});
    const onAuthChanged = (event: Event) => {
      clearTaskSnapshot();
      clearPageSnapshots();
      const detail = (event as CustomEvent<{ authenticated?: boolean }>).detail;
      authEpoch.current += 1;
      setCurrentUserId(null);
      setCurrentUser('');
      setAuthVersion(version => version + 1);
      setAuthNotice('');
      setAuthState(detail?.authenticated === false ? 'anonymous' : 'authed');
      setChatReady(false);
      setSection('chat');
    };
    window.addEventListener('aimychats:auth-changed', onAuthChanged);
    return () => {
      active = false;
      cleanExpired?.();
      window.removeEventListener('aimychats:auth-changed', onAuthChanged);
    };
  }, [checkAuth]);
  useEffect(() => {
    if (authState !== 'unavailable') return;
    const retry = () => checkAuth();
    const timer = window.setInterval(retry, 30000);
    window.addEventListener('online', retry);
    return () => { window.clearInterval(timer); window.removeEventListener('online', retry); };
  }, [authState, checkAuth]);
  const openNewUpload = () => { setStartTaskCreate(true); setSection('tasks'); };
  const finishWechatGuide = async (skipBackup = false) => {
    if (connectionFinishing) return;
    setConnectionFinishing(true); setConnectionFinishError('');
    try {
      const current = await invoke<Record<string, unknown>>('app_settings_get');
      const connectors = (current.connectors ?? {}) as Record<string, unknown>;
      await invoke('app_settings_save', { value: { ...current, connectors: { ...connectors, wechat_enabled: true },  } });
      if (skipBackup) { await invoke('connector_setup_state', { accountId: current.active_account_id, completed: true, skipBackup: true }); setWechatGuide(false); }
      if (skipBackup) { setFirstBackupStep('none'); setChatReady(true); setSection('chat'); }
      else { await invoke('connector_setup_state', { accountId: current.active_account_id, completed: false }); setWizard('wizard-sessions'); }
    } catch (error) { setConnectionFinishError(tf("无法启用微信连接：{0}", errorText(error))); }
    finally { setConnectionFinishing(false); }
  };
  // Keep the server's last-seen time fresh while this logged-in client is running.
  useEffect(() => {
    if (authState !== 'authed') return;
    const heartbeat = () => {
      const epoch = authEpoch.current;
      void registerDevifyDevice().then(async device => {
        if (!authMounted.current || epoch !== authEpoch.current) return;
        if (device) { setConnectionState('connected'); return; }
        setConnectionState('unavailable');
        try {
          const status = await invoke<{ authenticated: boolean; unavailable?: boolean }>('devify_auth_status');
          if (authMounted.current && epoch === authEpoch.current && !status.authenticated && !status.unavailable) {
            window.dispatchEvent(new CustomEvent('aimychats:auth-changed', { detail: { authenticated: false } }));
          }
        } catch { /* Retry on the next heartbeat. */ }
      });
    };
    heartbeat();
    const timer = window.setInterval(heartbeat, 2 * 60 * 1000);
    window.addEventListener('focus', heartbeat);
    return () => { window.clearInterval(timer); window.removeEventListener('focus', heartbeat); };
  }, [authState]);
  // Entitlements are per account and per server, so every successful sign-in refreshes them
  // once. A server without the endpoint answers 404 and the app keeps its legacy behaviour.
  useEffect(() => {
    if (authState !== 'authed') return;
    void refreshEntitlements();
  }, [authState]);
  const handleChatReady = useCallback((source: string) => { setChatSource(source); setChatReady(true); }, []);
  // Agent 集成 (local summaries plus the access kit) is reserved for internal accounts, so both
  // of its settings pages disappear with the entitlement instead of showing an empty shell.
  const { internal: internalAccount, view: entitlements } = useInternalAccount();
  const showAgentHub = internalAccount && (granted(entitlements, 'ai.local_agent') || granted(entitlements, 'bridge.agent_cli'));
  const [settingsPage, setSettingsPage] = useState<SettingsPage>('connectors');
  const [analysisFocus, setAnalysisFocus] = useState<'events'>();
  useEffect(() => {
    if ((settingsPage === 'agent' && !showAgentHub) || (settingsPage === 'history' && !internalAccount)) setSettingsPage('connectors');
  }, [settingsPage, showAgentHub, internalAccount]);
  const [wechatGuide, setWechatGuide] = useState(false);
  const [whatsappGuide, setWhatsappGuide] = useState(false);
  const runtime = useTaskRuntime();
  const [whatsappResume, setWhatsappResume] = useState<{ accountId: string; step: 'sessions' | 'backup' }>();
  const [wizardResuming, setWizardResuming] = useState(false);
  const wizardResumeEpoch = useRef(0);
  const [wechatGuideEdit, setWechatGuideEdit] = useState(false);
  const [wechatRecoveryAccount, setWechatRecoveryAccount] = useState<string | null>(null);
  const [wizardStep, setWizardStep] = useState('wizard-method');
  const [manualSelected, setWizardManual] = useState(false);
  const [wechatImportOnly, setWechatImportOnly] = useState(false);
  const wizardManual = manualSelected || wechatImportOnly;
  const wechatInstall = useConnectorDetect('wechat', { refreshOnFocus: !(section === 'settings' && settingsPage === 'connectors' && (wechatGuide || whatsappGuide)) });
  useEffect(() => {
    if (!wechatGuide) return;
    let active = true;
    void invoke<{ platform: string; supports_macos_capture: boolean }>('wechat_platform_capabilities')
      .then(value => { if (active) setWechatImportOnly(value.platform === 'macos' && !value.supports_macos_capture); })
      .catch(() => {});
    return () => { active = false; };
  }, [wechatGuide]);
  useEffect(() => {
    if (section === 'settings' && settingsPage === 'connectors' && wechatGuide && wizardStep === 'wizard-permission') {
      void wechatInstall.recheck();
    }
  }, [section, settingsPage, wechatGuide, wizardStep, wechatInstall.recheck]);
  const wizardBlocked = Boolean(wechatInstall.detect && !wechatInstall.canProceed);
  const wechatSignatureWarning = wechatInstall.detect?.state === 'untrusted';
  const wizardOrder = ['wizard-method', 'wizard-permission', 'wizard-connect', 'wizard-sessions', 'wizard-backup', 'wizard-done'];
  const settingsNav = settingsPages
    .filter(item => !(wechatGuide || whatsappGuide) || item.group === 'main' || item.id === 'feedback' || item.id === 'about')
    .filter(item => item.id === 'agent' ? showAgentHub : item.id !== 'history' || internalAccount);
  const openSettings = useCallback((page: SettingsPage, guide = false, edit = guide) => { wizardResumeEpoch.current += 1; setWizardResuming(false); setWechatRecoveryAccount(null); setAnalysisFocus(undefined); setSettingsPage(page); setWhatsappGuide(false); setWechatGuide(guide); setWechatGuideEdit(edit); setWizardManual(false); setWizardStep('wizard-method'); setSection('settings'); }, []);
  const handleChatUnlock = useCallback(() => openSettings('connectors', true), [openSettings]);
  const recoverWechatKeys = (accountId: string) => {
    openSettings('connectors', true);
    setWechatRecoveryAccount(accountId);
    setConnectionFinishError('');
  };
  const openWhatsApp = (resumeAccountId?: string) => {
    wizardResumeEpoch.current += 1; setWizardResuming(false);
    const source = runtime.sources?.find(source => source.account_id === resumeAccountId && !source.setup_complete);
    setWhatsappResume(source ? { accountId: source.account_id, step: source.wizard_step ?? 'sessions' } : undefined);
    setSettingsPage('connectors'); setWechatGuide(false); setWhatsappGuide(true); setSection('settings');
  };
  const resumeWechat = async (accountId: string) => {
    const epoch = authEpoch.current;
    const source = runtime.sources?.find(source => source.account_id === accountId && !source.setup_complete);
    if (!source) { openSettings('connectors', true); return; }
    openSettings('connectors', true); setWizardResuming(true); setConnectionFinishError('');
    const request = ++wizardResumeEpoch.current;
    const current = () => authEpoch.current === epoch && wizardResumeEpoch.current === request;
    try {
      const [settings, keys, capture] = await Promise.all([
        invoke<AppSettings>('app_settings_get'),
        invoke<{ keys: Array<{ account: string }> }>('wechat_key_status'),
        invoke<{ pending: boolean; needs_cleanup: boolean }>('wechat_capture_status'),
      ]);
      if (!current()) return;
      if (capture.pending || capture.needs_cleanup || !keys.keys.some(key => key.account === accountId)) {
        setWizardStep('wizard-connect'); setConnectionFinishError(t('连接微信的前置结果需要重新确认，请按提示处理后继续。')); return;
      }
      await invoke('chat_sessions', { accountId, limit: 1 });
      if (!current()) return;
      await invoke('chat_account_activate', { accountId });
      if (!current()) return;
      const selected = settings.chat.allowed_usernames[accountId]?.length ?? 0;
      setWizardStep(source.wizard_step === 'backup' && selected > 0 ? 'wizard-backup' : 'wizard-sessions');
    } catch (reason) {
      if (!current()) return;
      const permission = await invoke<{ granted: boolean }>('perm_status').catch(() => null);
      if (current()) {
        setWizardStep(permission?.granted === false ? 'wizard-permission' : 'wizard-connect');
        setConnectionFinishError(tf('未能恢复配置：{0}，请重新检查后继续。', errorText(reason)));
      }
    } finally { if (current()) setWizardResuming(false); }
  };
  const continueSetup = (platform: string) => {
    const source = runtime.sources?.find(source => source.platform === platform && !source.setup_complete);
    if (platform === 'whatsapp') openWhatsApp(source?.account_id); else if (source) void resumeWechat(source.account_id); else openSettings('connectors', true);
  };
  const finishWhatsApp = () => { setWhatsappGuide(false); setChatReady(false); setChatSource('whatsapp'); setChatKey(value => value + 1); setSection('chat'); };
  const settingsContentRef = useRef<HTMLDivElement | null>(null);
  const setWizard = (step: string) => setWizardStep(step);
  useLayoutEffect(() => {
    if (!wechatGuide && !whatsappGuide) return;
    settingsContentRef.current?.scrollTo({ top: 0, behavior: 'auto' });
  }, [wechatGuide, whatsappGuide, wizardStep]);
  const openEvent = (eventId: number) => { setEventFocusId(eventId); setSection('events'); };
  return <main className={`prototype-shell${DEVELOPMENT_APP ? ' prototype-shell-dev' : ''}${isTauri() && /Mac/.test(navigator.platform) ? ' prototype-shell-macos' : ''}`} onMouseDown={handleWindowDrag}>
    {DEVELOPMENT_APP && <DevelopmentEnvironment authenticated={authState === 'authed'} />}
    <div className="prototype-titlebar" aria-hidden="true" />
    <NavigationRail section={section} authenticated={authState === 'authed'} accountName={currentUser} onAccount={() => openSettings('account')}
      updateVersion={update.status === 'available' ? update.version : undefined}
      onUpdate={() => {
        setUpdateDialogOpen(true);
        // The rail button is rendered only for an already discovered release.
        // Rechecking here introduced a race with the download action and made a
        // valid update appear to do nothing when GitHub was slow.
        if (!['available', 'checking', 'downloading', 'installing', 'downloaded', 'installed'].includes(update.status)) {
          void checkForUpdates();
        }
      }}
      onNavigate={next => { if (next !== 'events') setEventFocusId(null); setStartTaskCreate(false); setSection(next); }} />
    <section className="prototype-content">
      {updateDialogOpen && <div className="auth-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) setUpdateDialogOpen(false); }}>
        <section className="auth-modal update-dialog" role="dialog" aria-modal="true" aria-labelledby="update-dialog-title">
          <header className="auth-modal-head"><span id="update-dialog-title">{t("软件更新")}</span><button type="button" aria-label={t("关闭更新")} autoFocus onClick={() => setUpdateDialogOpen(false)}>×</button></header>
          <div className="auth-modal-body"><UpdatePanel onClose={() => setUpdateDialogOpen(false)} /></div>
        </section>
      </div>}
      {authState === 'checking' && <div className="auth-checking" role="status"><span className="auth-gate-pulse"/>{t("正在检查登录状态…")}</div>}
      {authState === 'unavailable' && <div className="auth-gate">
        <div className="auth-gate-card">
          <span className="auth-gate-mark"><img src={brandMark} alt=""/></span>
          <h1>{t("暂时无法验证登录")}</h1>
          <p>{authError || t("服务端暂不可用，请稍后重试。")}</p>
          <p className="auth-gate-hint">{t("已保存的登录状态会保留，请根据上方提示处理后重试。")}</p>
          <button type="button" className="auth-gate-primary" disabled={authChecking || connectionSaving} aria-busy={authChecking} onClick={checkAuth}>{authChecking ? t("正在检查…") : t("重试")}</button>
          <button type="button" className="auth-gate-secondary" disabled={connectionSaving} onClick={() => { authEpoch.current += 1; setAuthNotice(''); setAuthState('anonymous'); }}>{t("返回登录")}</button>
          <button type="button" className="auth-gate-secondary" onClick={() => requestFeedback('login', 'login')}>{t("反馈此问题")}</button>
          <ConnectionSettingsDisclosure open={connectionSettingsOpen} disabled={authChecking || connectionSaving}
            onToggle={() => setConnectionSettingsOpen(open => !open)}>
            <NetworkConnectionControl compact disabled={authChecking} onSavingChange={setConnectionSaving} />
          </ConnectionSettingsDisclosure>
        </div>
      </div>}
      {authState === 'anonymous' && <LoginGate notice={authNotice} onAuthenticated={handleAuthenticated}/>}
      {authState === 'authed' && <>
      <div style={{ display: section === 'chat' ? 'contents' : 'none' }} aria-hidden={section !== 'chat'}>
        {chatReady && chatSource !== 'whatsapp' && !wechatInstall.checking && !wechatInstall.canProceed && wechatInstall.detect && <div className="chat-install-banner" role="status"><span className="connector-install-icon" aria-hidden="true">!</span><span><strong>{t(detectTitle[wechatInstall.detect.state])}{t("，已停止读取新消息。")}</strong>{t("已有记录仍可查看，重新安装后到「设置 › 连接器」点「重新检查」。")}</span><button type="button" onClick={() => openSettings('connectors')}>{t("去检查")}</button></div>}
        {!chatReady && firstBackupStep !== 'confirm' && <ChatEntry
          key={`entry-${chatKey}`}
          visible={section === 'chat'}
          onConnect={() => openSettings('connectors', true, false)}
          onConnectWhatsApp={() => void openWhatsApp()}
          onImport={() => { setFirstBackupStep('select'); openSettings('imports'); }}
          onBackup={openNewUpload}
          onSchedule={() => openSettings('backup')}
          onReady={handleChatReady}
          onUnlock={handleChatUnlock}
        />}
        {firstBackupStep === 'confirm' && <div className="chat-landing"><div className="chat-landing-inner first-backup-inner">
          <header className="chat-landing-head"><p className="chat-landing-eyebrow">{t("首次备份　·　最后一步")}</p><h1>{t("准备好首次备份")}</h1><p>{t("设置已保存，确认后就可以开始备份聊天记录。")}</p></header>
          <section className="first-backup-card" aria-label={t("备份设置")}>
            <div className="first-backup-card-head"><span className="first-backup-icon" aria-hidden="true">✓</span><div><strong>{t("备份设置已就绪")}</strong><p>{t("备份将在后台进行，你可以继续使用客户端。")}</p></div></div>
            <div className="first-backup-details"><div><span>{t("备份会话")}</span><strong>{backupPreview ? tf("已选择 {0} 个会话", (backupPreview.chat.allowed_usernames[backupPreview.active_account_id] ?? []).length) : t("正在读取…")}</strong></div><div><span>{t("自动备份")}</span><strong>{!backupPreview ? t("正在读取…") : !backupPreview.sync.enabled ? t("已关闭") : backupPreview.backup.mode === 'times' ? tf("每天 {0}", backupPreview.backup.times.join('、') || '未设置时间') : backupPreview.backup.mode === 'manual' ? t("仅手动") : tf("每 {0} 分钟", Math.round(backupPreview.chat.sync_interval_seconds / 60))}</strong></div><div><span>{t("网络条件")}</span><strong>{backupPreview ? (backupPreview.backup.wifi_only ? t("仅 Wi-Fi 或有线网络") : t("不限网络")) : t("正在读取…")}</strong></div></div>
            <div className="first-backup-actions"><button className="chat-landing-primary" onClick={() => { setStartFirstBackup(true); setFirstBackupStep('none'); setChatReady(true); }}>{t("立即开始首次备份")} <span aria-hidden="true">→</span></button><button className="first-backup-later" onClick={() => { setFirstBackupStep('none'); setChatReady(true); }}>{t("稍后自动备份")}</button></div>
          </section><p className="first-backup-note">{t("备份进度可以在「任务」中查看，首次备份时间取决于记录数量和网络情况。")}</p>
        </div></div>}
        {chatReady && firstBackupStep !== 'confirm' && <ChatWorkbench key={`workbench-${chatKey}`} visible={section === 'chat'} startFirstBackup={startFirstBackup} onFirstBackupStarted={() => setStartFirstBackup(false)} onOpenData={() => openSettings('connectors', true)} onRecoverWechatKeys={recoverWechatKeys} onOpenWhitelist={() => openSettings('imports')} onNewUpload={openNewUpload} onOpenTasks={() => { setStartTaskCreate(false); setSection('tasks'); }} onSchedule={() => openSettings('backup')} onOpenAccount={() => openSettings('account')} onOpenEvent={openEvent} onAddPlatform={() => openSettings('connectors')} />}
      </div>
      {section === 'tasks' && <Tasks startCreating={startTaskCreate} onNewUpload={() => openSettings('account')} onContinueSetup={continueSetup} />}
      {section === 'settings' && <div className="prototype-settings-layout">
        <header className="prototype-settings-breadcrumb">{t("设置")} <span>/</span> {t(settingsNav.find(item => item.id === settingsPage)?.label ?? '')}</header>
        <div className="prototype-settings">
        <aside className="prototype-settings-nav">
          <h1>{t("设置")}</h1>
          {settingsNav.map((item, index, list) => {
            const divider = item.group === 'local' && list[index - 1]?.group === 'main';
            return <Fragment key={item.id}>
              {divider && <div className="prototype-settings-divider" aria-hidden="true"/>}
              <button type="button" className={settingsPage === item.id ? 'active' : ''} onClick={() => { wizardResumeEpoch.current += 1; setWizardResuming(false); setSettingsPage(item.id); setAnalysisFocus(undefined); setWechatGuide(false); setWhatsappGuide(false); }} aria-current={settingsPage === item.id ? 'page' : undefined}><span className="settings-nav-icon">{settingsIcons[item.id]}</span>{t(item.label)}</button>
            </Fragment>;
          })}
        </aside>
        <div className="prototype-settings-content" ref={settingsContentRef}>
          {settingsPage === 'connectors' && (whatsappGuide ? <WhatsAppWizard resume={whatsappResume} onBack={() => setWhatsappGuide(false)} onOpenChat={finishWhatsApp} /> : wechatGuide && wizardResuming ? <p role="status">{t('正在恢复上次的配置…')}</p> : wechatGuide ? <div className="wechat-wizard">
            <button type="button" className="connector-back" onClick={() => setWechatGuide(false)}>{t("← 返回连接器")}</button>
            <header className="wizard-connector-head"><span className="wizard-connector-mark" aria-hidden="true"><img src={wechatIcon} alt="" /></span><div><strong>{wechatGuideEdit ? t("重新配置") : t("连接向导")}</strong><h2>{t("微信")}</h2><p>{t("读取本机数据库，只读，不改动微信")}</p></div></header>
            <nav className="wizard-stepper" aria-label={t("微信连接向导")}>{[
              ['wizard-method',t("接入方式")], ['wizard-permission',t("系统检查")], ['wizard-connect',t("连接微信")], ['wizard-sessions',t("选择会话")], ['wizard-backup',t("备份周期")], ['wizard-done',t("完成")],
            ].map(([id,label], index) => { const current = wizardOrder.indexOf(wizardStep); const complete = index < current; return <button key={id} type="button" className={`${wizardStep === id ? 'active ' : ''}${complete ? 'complete' : ''}`} aria-current={wizardStep === id ? 'step' : undefined} onClick={() => setWizard(id)} disabled={index > current || wizardBlocked}><span>{complete ? Icons.check : index + 1}</span>{t(label)}</button>; })}</nav>
            {wizardStep === 'wizard-method' && (
            <section id="wizard-method" className="wizard-method"><div><h2>{t("选择接入方式")}</h2><p>{wechatImportOnly ? t("当前系统使用导入模式") : t("两种方式都只读取聊天记录，也不需要你的微信密码。")}</p></div>{wizardBlocked && wechatInstall.detect && <div className="wizard-install-block" role="alert"><span className="connector-install-icon" aria-hidden="true">!</span><div><strong>{t(detectTitle[wechatInstall.detect.state])}{t("，暂时无法继续")}</strong><p>{t(wechatInstall.detect.message)}</p><div className="wizard-install-actions"><button type="button" className="connector-quiet" onClick={() => void wechatInstall.recheck()} disabled={wechatInstall.checking}>{t("重新检测")}</button>{wechatInstall.detect.state === 'not_installed' && <a className="connector-link" href="https://pc.weixin.qq.com/" target="_blank" rel="noreferrer">{t("前往下载微信 ↗")}</a>}</div></div></div>}{wechatSignatureWarning && <p className="wizard-install-warning" role="status">{t('当前微信签名待确认，可手动导入已有密钥；自动连接前请确认或恢复官方微信。')}</p>}{!wizardBlocked && !wechatSignatureWarning && wechatInstall.detect?.version && <p className="wizard-install-ok"><span className="good" />{t("已检测到微信")}{wechatInstall.detect.version} {t("· 官方签名")}</p>}<div className="wizard-method-options">{!wechatImportOnly && <button type="button" className={`wizard-method-option${!wizardManual ? ' selected' : ''}`} aria-pressed={!wizardManual} disabled={wizardBlocked} onClick={() => setWizardManual(false)}><strong>{t("自动读取本机数据库")}<em>{t("推荐")}</em></strong><span>{t("完成系统检查后，按步骤连接微信，读取本机聊天记录。")}</span></button>}<button type="button" className={`wizard-method-option${wizardManual ? ' selected' : ''}`} aria-pressed={wizardManual} disabled={wizardBlocked} onClick={() => setWizardManual(true)}><strong>{t("手动导入微信密钥")}{!wechatImportOnly && <em>{t("已有密钥时使用")}</em>}</strong><span>{t("选择密钥文件或直接粘贴，校验后读取本机聊天记录。")}</span></button></div><p className="wizard-method-note">{t("仅用于读取与校验，不会修改微信数据库。")}</p><div className="wizard-method-footer"><span className="wizard-next-hint">{wizardBlocked ? t("安装并检测通过后才能继续") : t("接下来：系统检查")}</span><button type="button" className="wizard-next" disabled={wizardBlocked} onClick={() => setWizard('wizard-permission')}>{t("下一步")}</button></div></section>
            )}
            <div id="wizard-permission" className="wizard-anchor" /><div id="wizard-account" className="wizard-anchor" /><div id="wizard-key" className="wizard-anchor" /><div id="wizard-import" className="wizard-anchor" /><div id="wizard-done" className="wizard-anchor" />
            {wizardStep === 'wizard-sessions' ? <section className="wizard-settings-step">
              <WhitelistSettings wizard onBack={() => setWizard('wizard-connect')} onSaved={async count => { if (count > 0) { try { const current = await invoke<AppSettings>('app_settings_get'); await invoke('connector_setup_state', { accountId: current.active_account_id, completed: false, wizardStep: 'backup' }); setWizard('wizard-backup'); } catch (reason) { setConnectionFinishError(tf('保存向导进度失败：{0}，请重试。', errorText(reason))); } } }} />
              <button type="button" className="wizard-skip" disabled={connectionFinishing} onClick={() => void finishWechatGuide(true)}>{t("暂不备份，进入聊天")}</button>
            </section> : wizardStep === 'wizard-backup' ? <section className="wizard-settings-step">
              <BackupSchedule wizard onBack={() => setWizard('wizard-sessions')} onOpenConnectors={() => setWizard('wizard-connect')} onContinue={async () => { const current = await invoke<AppSettings>('app_settings_get'); await invoke('connector_setup_state', { accountId: current.active_account_id, completed: true }); setWizard('wizard-done'); }} />
            </section> : wizardStep === 'wizard-done' ? <WeChatWizardComplete edit={wechatGuideEdit} onBack={() => setWizard('wizard-connect')} onFinish={() => { setWechatGuide(false); setFirstBackupStep('none'); setChatReady(true); setChatKey(value => value + 1); setSection('chat'); }} /> : <WeChatLocal mode="settings" wizard wizardStep={wizardStep} wizardManual={wizardManual} wizardFinishing={connectionFinishing} accountId={wechatRecoveryAccount}
              wizardInstall={{ ready: wechatInstall.checking ? null : wechatInstall.canProceed, message: wechatInstall.detect?.message ?? '' }} onWizardInstallRecheck={() => wechatInstall.recheck()}
              onWizardStep={step => {
                if (step !== 'wizard-sessions') { setWizard(step); return; }
                if (wechatRecoveryAccount) { setWechatGuide(false); setWechatRecoveryAccount(null); setSection('chat'); }
                else void finishWechatGuide();
              }} onOpenChat={() => setSection('chat')} />}
            {connectionFinishError && <p className="wizard-save-error" role="alert">{connectionFinishError}</p>}
          </div> : <Connectors onOpenWechat={(configured, resumeAccountId) => { if (resumeAccountId) { void resumeWechat(resumeAccountId); return; } openSettings('connectors', true, configured); }} onOpenWhatsApp={openWhatsApp} />)}
          {settingsPage === 'imports' && <WhitelistSettings onSaved={count => { if (firstBackupStep === 'select' && count > 0) { setFirstBackupStep('schedule'); setSettingsPage('backup'); } }} />}
          {settingsPage === 'backup' && <BackupSchedule onOpenConnectors={() => { setWechatGuide(false); setSettingsPage('connectors'); }} onContinue={firstBackupStep === 'schedule' ? () => { setFirstBackupStep('confirm'); setSection('chat'); } : undefined} />}
          {settingsPage === 'analysis' && <AnalysisSettings focusSection={analysisFocus} onOpenReports={() => setSection('reports')} onOpenTopics={() => setSection('topics')} onOpenChats={() => setSection('chat')} canUseLocalSummary={showAgentHub && granted(entitlements, 'ai.local_agent') && granted(entitlements, 'ai.agent_summary')} />}
          {settingsPage === 'language' && <LanguageSettings />}
          {settingsPage === 'network' && <NetworkSettings />}
          {settingsPage === 'account' && <DevifySettings initialUser={currentUser} initialAuthenticated={authState === 'authed'} connection={connectionState} />}
          {settingsPage === 'history' && internalAccount && <HistoryPage />}
          {settingsPage === 'agent' && showAgentHub && <AgentHub onOpenAnalysis={() => setSettingsPage('analysis')} />}
          {settingsPage === 'feedback' && <FeedbackSettings />}
          {settingsPage === 'about' && <About />}
        </div>
        </div>
      </div>}
      {section === 'events' && <CloudAnalysisPage kind="events" userId={currentUserId} focusId={eventFocusId} onOpenEvent={openEvent} onOpenAnalysisSettings={() => { openSettings('analysis'); setAnalysisFocus('events'); }} />}
      {section === 'reports' && <CloudAnalysisPage kind="reports" userId={currentUserId} />}
      {section === 'topics' && <TopicsPage userId={currentUserId} onOpenEvent={openEvent} />}
      {section === 'ask' && <AskPage onOpenEvent={openEvent} />}
      </>}
    </section>
    {feedbackRequest && <Feedback category={feedbackRequest.category} page={feedbackRequest.page} relatedBatchIds={feedbackRequest.relatedBatchIds} onClose={() => setFeedbackRequest(null)} />}
  </main>;
}
