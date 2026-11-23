import { t, tf, ctf } from '../../shared/i18n/locale';
import { useCallback, useEffect, useRef, useState } from 'react';
import { desktopInvoke as invoke } from '../../shared/api/desktopInvoke';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import brandMark from '../../assets/app-mark-transparent.png';
import { isSettingsConflict, requireSettingsRevision, settingsSaveError } from '../settings/settingsModel';
import { apiBaseUrl, storedBaseUrl } from '../../shared/api/serviceUrl.ts';
import { requestFeedback } from '../../shared/api/desktopInvoke';
import { NetworkConnectionControl } from '../settings/NetworkSettings';

type GateSettings = {
  user_settings_revision: number;
  devify?: { base_url?: string; device_label?: string };
  [key: string]: unknown;
};

type Tab = 'login' | 'register';

/** Example only: the app ships without a server, so this is never used as a value. */
const SERVER_PLACEHOLDER = 'https://devify.example.com';

export function LoginGate({ notice = '', onAuthenticated }: { notice?: string; onAuthenticated: () => void }) {
  const [popup, setPopup] = useState(false);
  const [tab, setTab] = useState<Tab>('login');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState<'' | 'password' | 'browser-config' | 'browser-opening' | 'browser' | 'session'>('');
  const [connectionSaving, setConnectionSaving] = useState(false);
  const [error, setError] = useState('');
  const [info, setInfo] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [serverOpen, setServerOpen] = useState(false);
  const [serverDraft, setServerDraft] = useState('');
  const [serverStatus, setServerStatus] = useState('');
  const completed = useRef(false);
  const mounted = useRef(true);
  const sessionCheckLock = useRef(false);
  const loginActionLock = useRef(false);
  const settingsRef = useRef<GateSettings | null>(null);
  const draftRef = useRef('');
  const saveLock = useRef(false);
  const conflict = useRef(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [serverSaving, setServerSaving] = useState(false);
  const dialogRef = useRef<HTMLElement | null>(null);
  const loginButtonRef = useRef<HTMLButtonElement | null>(null);

  const finishBrowserLogin = useCallback(async (url: string) => {
    try {
      await invoke('devify_auth_callback', { callbackUrl: url });
      if (completed.current) return;
      completed.current = true;
      setBusy('');
      onAuthenticated();
    } catch (reason) {
      setBusy('');
      setInfo('');
      setError(String(reason));
    }
  }, [onAuthenticated]);

  useEffect(() => {
    let active = true;
    let cleanDeepLink: (() => void) | undefined;
    let cleanLoopback: (() => void) | undefined;
    void onOpenUrl(urls => {
      for (const url of urls) void finishBrowserLogin(url);
    }).then(unlisten => { if (active) cleanDeepLink = unlisten; else unlisten(); }).catch(() => {});
    void listen<string>('devify-auth-loopback', event => {
      void finishBrowserLogin(event.payload);
    }).then(unlisten => { if (active) cleanLoopback = unlisten; else unlisten(); }).catch(() => {});
    return () => { active = false; cleanDeepLink?.(); cleanLoopback?.(); };
  }, [finishBrowserLogin]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      // A completed login must retain its generation. An abandoned browser
      // attempt still releases its loopback listener and invalidates callbacks.
      if (!completed.current) void invoke('devify_auth_cancel').catch(() => {});
    };
  }, []);

  useEffect(() => {
    void invoke<GateSettings>('app_settings_get')
      .then(stored => {
        requireSettingsRevision(stored);
        settingsRef.current = stored;
        setSettingsLoaded(true);
        const value = storedBaseUrl(stored.devify?.base_url ?? '').replace(/\/+$/, '');
        draftRef.current = value;
        setBaseUrl(value);
        setServerDraft(value);
        // A fresh install has no server, so the field is required instead of hidden.
        if (!value) setServerOpen(true);
      })
      .catch(reason => setError(tf("设置加载失败：{0}，请重新打开页面。", String(reason))));
  }, []);

  useEffect(() => {
    if (!popup) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy && !connectionSaving) { setPopup(false); setError(''); setInfo(''); }
      if (event.key !== 'Tab') return;
      const controls = dialogRef.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), [tabindex="0"]');
      if (!controls?.length) return;
      const first = controls[0], last = controls[controls.length - 1];
      if (!dialogRef.current?.contains(document.activeElement)) { event.preventDefault(); (event.shiftKey ? last : first).focus(); }
      else if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [popup, busy, connectionSaving]);
  useEffect(() => {
    if (!popup) return;
    return () => loginButtonRef.current?.focus();
  }, [popup]);

  // The browser flow has no deadline of its own: when the server is unreachable the
  // callback never arrives, so stop waiting instead of leaving the modal spinning.
  useEffect(() => {
    if (busy !== 'browser') return;
    const timer = window.setTimeout(() => {
      void invoke('devify_auth_cancel').catch(() => {});
      setBusy('');
      setInfo('');
      setError(t("等待浏览器授权超时，请确认服务端可访问后重试。"));
    }, 120_000);
    return () => window.clearTimeout(timer);
  }, [busy]);

  const idOk = /^[a-z0-9._-]{3,}$/i.test(username);
  const browserStarting = busy === 'browser-config' || busy === 'browser-opening';
  const configFailed = /^(读取服务端登录配置失败|服务端登录配置)/.test(error);
  const retrySavedSession = async () => {
    if (sessionCheckLock.current || busy !== '' || connectionSaving) return;
    sessionCheckLock.current = true;
    setBusy('session');
    setError('');
    try {
      const status = await invoke<{ authenticated: boolean; unavailable?: boolean; error?: string }>('devify_auth_status');
      if (!mounted.current) return;
      if (status.authenticated) {
        if (completed.current) return;
        completed.current = true;
        onAuthenticated();
      } else {
        setError(status.unavailable ? status.error || '暂时无法验证登录，请稍后重试。' : '没有可用的已保存登录，请重新登录。');
      }
    } catch (reason) { if (mounted.current) setError(String(reason)); }
    finally { sessionCheckLock.current = false; if (mounted.current) setBusy(''); }
  };
  const submitLogin = async () => {
    setBusy('password');
    setError('');
    setInfo('');
    try {
      await invoke('devify_login_password', { username, password });
      if (!mounted.current || completed.current) return;
      completed.current = true;
      onAuthenticated();
    } catch (reason) {
      if (mounted.current) setError(String(reason));
    } finally {
      if (mounted.current) setBusy('');
    }
  };

  /** Reject anything that cannot be used as an http(s) request base. */
  const serverError = (value: string): string | null => {
    try {
      const parsed = new URL(value);
      if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
        return t("服务地址必须以 http:// 或 https:// 开头。");
      }
      if (!parsed.hostname) return t("服务地址缺少主机名，请填写完整地址。");
    } catch {
      return t("请输入完整的服务地址，例如 https://devify.example.com。");
    }
    return null;
  };

  const persistServer = async (value: string): Promise<boolean> => {
    if (!settingsRef.current || saveLock.current || conflict.current) return false;
    saveLock.current = true;
    setServerSaving(true);
    const draftAtStart = draftRef.current;
    try {
      const stored = settingsRef.current;
      const next: GateSettings = { ...stored, devify: { ...stored.devify, base_url: storedBaseUrl(value) } };
      const saved = requireSettingsRevision(await invoke<GateSettings>('app_settings_save', { value: next }));
      settingsRef.current = saved;
      const confirmed = storedBaseUrl(saved.devify?.base_url ?? '').replace(/\/+$/, '');
      setBaseUrl(confirmed);
      if (draftRef.current !== draftAtStart) {
        setServerStatus(t("之前的地址已保存，新的输入尚未保存，请确认后重试。"));
        return false;
      }
      draftRef.current = confirmed;
      setServerDraft(confirmed);
      setError('');
      return true;
    } catch (reason) {
      if (isSettingsConflict(reason)) conflict.current = true;
      setServerStatus(settingsSaveError(reason));
      return false;
    } finally {
      saveLock.current = false;
      setServerSaving(false);
    }
  };

  /**
   * Every action that talks to Devify needs a server first: login, browser login and
   * the register/forgot-password links all validate and persist the address, then
   * continue with the normalized value.
   */
  const ensureServer = async (): Promise<string | null> => {
    if (conflict.current) { setError(t("设置已变化，请重新打开页面并确认账号和服务地址。")); return null; }
    if (!settingsRef.current || saveLock.current) { setError(t("设置尚未加载或正在保存，请稍后重试。")); return null; }
    const current = baseUrl.trim().replace(/\/+$/, '');
    const candidate = serverDraft.trim().replace(/\/+$/, '') || current;
    if (!candidate) {
      setServerOpen(true);
      setServerStatus(t("请先填写服务端地址，再继续登录。"));
      setError(t("请先填写服务端地址，再继续登录。"));
      return null;
    }
    const invalid = serverError(candidate);
    if (invalid) {
      setServerOpen(true);
      setServerStatus(invalid);
      setError(invalid);
      return null;
    }
    if (candidate === current) return candidate;
    if (!(await persistServer(candidate))) {
      setServerOpen(true);
      setError(t("服务地址保存失败，请检查后重试。"));
      return null;
    }
    setServerOpen(false);
    setServerStatus(t("服务地址已保存。"));
    return candidate;
  };

  const submitPassword = async () => {
    if (loginActionLock.current || busy || connectionSaving) return;
    loginActionLock.current = true;
    setBusy('password');
    try {
      if (!(await ensureServer())) return;
      if (!idOk) { setError(t("请输入用户名（至少 3 位，可用字母、数字和 . _ -）")); return; }
      if (!password) { setError(t("请输入密码")); return; }
      await submitLogin();
    } finally {
      loginActionLock.current = false;
      if (mounted.current) setBusy('');
    }
  };

  const cancelBrowserLogin = async () => {
    try { await invoke('devify_auth_cancel'); } catch { /* the previous flow may already be gone */ }
    setBusy('');
    setInfo('');
  };

  const startBrowserLogin = async () => {
    if (loginActionLock.current || busy || connectionSaving) return;
    loginActionLock.current = true;
    setBusy('browser-config');
    setError('');
    setInfo('');
    try {
      if (!(await ensureServer())) { setBusy(''); return; }
      const settings = await invoke<GateSettings>('app_settings_get');
      const deviceLabel = settings.devify?.device_label?.trim() || 'AIMyChats';
      const url = await invoke<string>('devify_auth_start', { deviceLabel });
      setBusy('browser-opening');
      await invoke('devify_auth_open', { url });
      setBusy('browser');
      setInfo(t("已在浏览器打开登录页，完成授权后会自动返回。"));
    } catch (reason) {
      await invoke('devify_auth_cancel').catch(() => {});
      setBusy('');
      setInfo('');
      const message = String(reason);
      setError(/上传期间无法更改登录状态|备份正在使用登录状态/.test(message)
        ? t("后台备份尚未结束，暂时无法登录。如果长时间没有进展，请退出并重新打开 AIMyChats 后重试。")
        : message);
    } finally { loginActionLock.current = false; }
  };

  const openExternal = async (path: string) => {
    setError('');
    const server = await ensureServer();
    if (!server) return;
    try {
      await invoke('devify_auth_open', { url: `${apiBaseUrl(server)}${path}` });
    } catch (reason) {
      setError(String(reason));
    }
  };

  const saveServer = async () => {
    if (conflict.current) { setServerStatus(t("设置已变化，请重新打开页面并确认账号和服务地址。")); return; }
    const value = serverDraft.trim().replace(/\/+$/, '');
    if (!value) {
      setServerStatus(t("请填写服务端地址。"));
      return;
    }
    const invalid = serverError(value);
    if (invalid) {
      setServerStatus(invalid);
      return;
    }
    if (!(await persistServer(value))) return;
    setServerOpen(false);
    setServerStatus(t("服务地址已保存。"));
  };

  return <div className="auth-gate auth-login-gate">
    <div className="auth-gate-card" aria-hidden={popup || undefined} {...(popup ? { inert: '' } : {})}>
      <span className="auth-gate-mark"><img src={brandMark} alt=""/></span>
      <h1>{t("需要登录")}</h1>
      <p>{t("登录后才能连接聊天来源、备份和使用智能分析。")}</p>
      {notice && <p className="auth-gate-error" role="alert">{notice}</p>}
      {busy === 'browser'
        ? <div className="auth-gate-waiting" role="status"><span className="auth-gate-pulse"/><span>{t("正在等待浏览器完成授权…")}</span></div>
        : <button ref={loginButtonRef} type="button" className="auth-gate-primary" disabled={!settingsLoaded || serverSaving || connectionSaving || busy !== ''} onClick={() => {
            if (!settingsLoaded) return;
            if (!baseUrl.trim()) {
              setServerOpen(true);
              setServerStatus(t("请先填写服务端地址，再继续登录。"));
              return;
            }
            setPopup(true); setError(''); setInfo('');
          }}>{t("登录…")}</button>}
      {busy === 'browser' && <button type="button" className="auth-gate-secondary" onClick={() => void cancelBrowserLogin()}>{t("取消")}</button>}
      {busy !== 'browser' && <button type="button" className="auth-gate-secondary" disabled={!settingsLoaded || serverSaving || connectionSaving || busy !== ''} aria-busy={busy === 'session'} onClick={() => void retrySavedSession()}>{busy === 'session' ? t("正在检查…") : t("重新检查已有登录")}</button>}
      {error && !popup && <p className="auth-gate-error" role="alert">{error} <button type="button" onClick={() => requestFeedback('login', 'login')}>{t("反馈此问题")}</button></p>}
      <div className="auth-gate-server">
        {serverOpen || !baseUrl
          ? <>
              <label>{baseUrl ? t("服务端地址") : t("服务端地址（首次使用请填写）")}
                <input
                  type="url"
                  disabled={!settingsLoaded}
                  value={serverDraft}
                  autoFocus={!baseUrl}
                  onChange={event => { draftRef.current = event.target.value; setServerDraft(event.target.value); }}
                  onKeyDown={event => { if (event.key === 'Enter') void saveServer(); }}
                  placeholder={SERVER_PLACEHOLDER}
                />
              </label>
              {!baseUrl && <p className="auth-gate-hint">{t("服务端不随客户端分发，请填写你们部署的服务端地址。")}</p>}
              <div className="auth-gate-server-actions">
                <button type="button" disabled={!settingsLoaded || serverSaving} onClick={() => void saveServer()}>{t("保存")}</button>
                {baseUrl && <button type="button" onClick={() => { setServerOpen(false); draftRef.current = baseUrl; setServerDraft(baseUrl); if (!conflict.current) setServerStatus(''); }}>{t("取消")}</button>}
              </div>
            </>
          : <button type="button" className="auth-gate-server-toggle" onClick={() => { setServerOpen(true); draftRef.current = baseUrl; setServerDraft(baseUrl); if (!conflict.current) setServerStatus(''); }}>{t("服务地址 ·")} {storedBaseUrl(baseUrl).replace(/^https?:\/\//, '').replace(/\/+$/, '')}</button>}
      </div>
      {serverStatus && <p className="auth-gate-hint" role="status">{serverStatus}</p>}
      {!popup && <NetworkConnectionControl compact disabled={busy !== ''} onSavingChange={setConnectionSaving} />}
    </div>

    {popup && <div className="auth-backdrop auth-login-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget && !busy && !connectionSaving) { setPopup(false); setError(''); setInfo(''); } }}>
      <section ref={dialogRef} className="auth-modal auth-login-modal" role="dialog" aria-modal="true" aria-labelledby="auth-modal-title">
        <header className="auth-modal-head">
          <span className="auth-modal-brand"><img src={brandMark} alt=""/><span id="auth-modal-title">{t("登录 Aimychats")}</span></span>
          <button type="button" aria-label={t("关闭登录")} disabled={busy !== '' || connectionSaving} onClick={() => { setPopup(false); setError(''); setInfo(''); }}><svg viewBox="0 0 20 20" aria-hidden="true"><path d="m6 6 8 8M14 6l-8 8"/></svg></button>
        </header>
        <form className="auth-modal-body" onSubmit={event => { event.preventDefault(); if (tab === 'login' && !busy) void submitPassword(); }}>
          <div className="auth-tabs" role="tablist" aria-label={t("登录方式")}>
            {([['login', t("登录")], ['register', t("注册")]] as Array<[Tab, string]>).map(([key, label]) => <button
              key={key} id={`auth-${key}-tab`} type="button" role="tab" aria-selected={tab === key} aria-controls={`auth-${key}-panel`} disabled={busy !== '' || connectionSaving} className={tab === key ? 'active' : ''}
              onClick={() => { setTab(key); setError(''); setInfo(''); }}
            >{t(label)}</button>)}
          </div>

          {tab === 'login' && <div className="auth-login-fields" id="auth-login-panel" role="tabpanel" aria-labelledby="auth-login-tab">
            <div className="auth-browser-option">
              <button type="button" className="auth-alt" disabled={busy !== '' || connectionSaving} aria-busy={browserStarting} onClick={() => void startBrowserLogin()}>
                {browserStarting ? <span className="auth-button-spinner" aria-hidden="true"/> : <svg className="auth-alt-mark" viewBox="0 0 20 20" aria-hidden="true"><path d="M11 3h6v6M17 3l-8 8M8 4H4a1 1 0 0 0-1 1v11a1 1 0 0 0 1 1h11a1 1 0 0 0 1-1v-4"/></svg>}
                {busy === 'browser-config' ? t("正在读取登录配置…") : busy === 'browser-opening' ? t("正在打开浏览器…") : t("通过浏览器登录")}
              </button>
              <p className="auth-alt-help">{t("在浏览器完成登录后，将自动返回客户端。")}</p>
              <NetworkConnectionControl compact disabled={busy !== ''} onSavingChange={setConnectionSaving} />
              {configFailed && <p className="auth-alt-help">{t("可切换连接方式后重试。")}</p>}
            </div>
            <div className="auth-divider"><span>{t("或")}</span></div>
            <label className="auth-field">{t("AI 邮箱用户名")}
              <span className="auth-input-row">
                <input name="username" value={username} disabled={busy !== '' || connectionSaving} autoFocus placeholder={t("用户名")} autoComplete="username" autoCapitalize="none" spellCheck={false} onChange={event => { setUsername(event.target.value.trim()); setError(''); }}/>
                <span className="auth-suffix">@aimychats.com</span>
              </span>
            </label>
            <label className="auth-field">{t("密码")}
              <span className="auth-input-row">
                <input name="password" type={showPassword ? 'text' : 'password'} value={password} disabled={busy !== '' || connectionSaving} placeholder={t("密码")} autoComplete="current-password" onChange={event => { setPassword(event.target.value); setError(''); }}/>
                <button type="button" className="auth-show" disabled={busy !== '' || connectionSaving} aria-pressed={showPassword} onClick={() => setShowPassword(value => !value)}>{showPassword ? t("隐藏") : t("显示")}</button>
              </span>
            </label>
            <div className="auth-forgot"><button type="button" disabled={busy !== '' || connectionSaving} onClick={() => void openExternal('/login?forgot=1')}>{t("忘记密码？")}</button></div>
            {error && <p className="auth-error" role="alert">{t(error)} <button type="button" onClick={() => requestFeedback('login', 'login')}>{t("反馈此问题")}</button></p>}
            {info && busy !== 'browser' && <p className="auth-info" role="status">{info}</p>}
            {busy === 'browser' && <div className="auth-waiting" role="status"><span className="auth-gate-pulse"/><span>{t("正在等待浏览器完成授权…")}</span><button type="button" onClick={() => void cancelBrowserLogin()}>{t("取消")}</button></div>}
            <button type="submit" className="auth-submit" disabled={busy !== '' || connectionSaving} aria-busy={busy === 'password'}>{busy === 'password' && <span className="auth-button-spinner" aria-hidden="true"/>}{busy === 'password' ? t("正在登录…") : t("登录")}</button>
            <p className="auth-switch">{t("还没有账号？")}<button type="button" disabled={busy !== '' || connectionSaving} onClick={() => { setTab('register'); setError(''); setInfo(''); }}>{t("注册")}</button></p>
          </div>}

          {tab === 'register' && <div className="auth-login-fields" id="auth-register-panel" role="tabpanel" aria-labelledby="auth-register-tab">
            <div className="auth-register">
              <strong>{t("注册需要先验证邮箱")}</strong>
              <p>{t("在浏览器中创建 Aimychats 账号：填写邮箱并完成验证后，设置 AI 邮箱用户名和密码，再回到客户端登录。")}</p>
              <button type="button" className="auth-submit" onClick={() => void openExternal('/register')}>{t("在浏览器中注册")}</button>
            </div>
            {error && <p className="auth-error" role="alert">{t(error)}</p>}
            <p className="auth-switch">{t("已有账号？")}<button type="button" onClick={() => { setTab('login'); setError(''); setInfo(''); }}>{t("登录")}</button></p>
          </div>}
        </form>
      </section>
    </div>}
  </div>;
}
