import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import appIcon from './assets/app-icon.png';
import { isSettingsConflict, requireSettingsRevision, settingsSaveError } from './Settings';
import { apiBaseUrl, displayBaseUrl } from './serviceUrl.ts';

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
  const [busy, setBusy] = useState<'' | 'password' | 'browser'>('');
  const [error, setError] = useState('');
  const [info, setInfo] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [serverOpen, setServerOpen] = useState(false);
  const [serverDraft, setServerDraft] = useState('');
  const [serverStatus, setServerStatus] = useState('');
  const completed = useRef(false);
  const settingsRef = useRef<GateSettings | null>(null);
  const draftRef = useRef('');
  const saveLock = useRef(false);
  const conflict = useRef(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [serverSaving, setServerSaving] = useState(false);

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

  useEffect(() => () => {
    // Leaving the gate (or closing the app) must release the loopback port so
    // the next login attempt is not blocked by a stale listener.
    void invoke('devify_auth_cancel').catch(() => {});
  }, []);

  useEffect(() => {
    void invoke<GateSettings>('app_settings_get')
      .then(stored => {
        requireSettingsRevision(stored);
        settingsRef.current = stored;
        setSettingsLoaded(true);
        const value = displayBaseUrl(stored.devify?.base_url?.trim().replace(/\/+$/, '') || '').replace(/\/+$/, '');
        draftRef.current = value;
        setBaseUrl(value);
        setServerDraft(value);
        // A fresh install has no server, so the field is required instead of hidden.
        if (!value) setServerOpen(true);
      })
      .catch(reason => setError(`设置加载失败：${String(reason)}，请重新打开页面。`));
  }, []);

  useEffect(() => {
    if (!popup) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && busy !== 'browser') { setPopup(false); setError(''); setInfo(''); }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [popup, busy]);

  // The browser flow has no deadline of its own: when the server is unreachable the
  // callback never arrives, so stop waiting instead of leaving the modal spinning.
  useEffect(() => {
    if (busy !== 'browser') return;
    const timer = window.setTimeout(() => {
      void invoke('devify_auth_cancel').catch(() => {});
      setBusy('');
      setInfo('');
      setError('等待浏览器授权超时，请确认服务端可访问后重试。');
    }, 120_000);
    return () => window.clearTimeout(timer);
  }, [busy]);

  const idOk = /^[a-z0-9._-]{3,}$/i.test(username);
  const submitLogin = async () => {
    setBusy('password');
    setError('');
    setInfo('');
    try {
      await invoke('devify_login_password', { username, password });
      if (completed.current) return;
      completed.current = true;
      onAuthenticated();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy('');
    }
  };

  /** Reject anything that cannot be used as an http(s) request base. */
  const serverError = (value: string): string | null => {
    try {
      const parsed = new URL(value);
      if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
        return '服务地址必须以 http:// 或 https:// 开头。';
      }
      if (!parsed.hostname) return '服务地址缺少主机名，请填写完整地址。';
    } catch {
      return '请输入完整的服务地址，例如 https://devify.example.com。';
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
      const next: GateSettings = { ...stored, devify: { ...stored.devify, base_url: displayBaseUrl(value) || value } };
      const saved = requireSettingsRevision(await invoke<GateSettings>('app_settings_save', { value: next }));
      settingsRef.current = saved;
      const confirmed = displayBaseUrl(saved.devify?.base_url?.trim().replace(/\/+$/, '') || '').replace(/\/+$/, '');
      setBaseUrl(confirmed);
      if (draftRef.current !== draftAtStart) {
        setServerStatus('之前的地址已保存，新的输入尚未保存，请确认后重试。');
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
    if (conflict.current) { setError('设置已变化，请重新打开页面并确认账号和服务地址。'); return null; }
    if (!settingsRef.current || saveLock.current) { setError('设置尚未加载或正在保存，请稍后重试。'); return null; }
    const current = baseUrl.trim().replace(/\/+$/, '');
    const candidate = serverDraft.trim().replace(/\/+$/, '') || current;
    if (!candidate) {
      setServerOpen(true);
      setServerStatus('请先填写服务端地址，再继续登录。');
      setError('请先填写服务端地址，再继续登录。');
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
      setError('服务地址保存失败，请检查后重试。');
      return null;
    }
    setServerOpen(false);
    setServerStatus('服务地址已保存。');
    return candidate;
  };

  const submitPassword = async () => {
    if (!(await ensureServer())) return;
    if (!idOk) { setError('请输入用户名（至少 3 位，可用字母、数字和 . _ -）'); return; }
    if (!password) { setError('请输入密码'); return; }
    await submitLogin();
  };

  const cancelBrowserLogin = async () => {
    try { await invoke('devify_auth_cancel'); } catch { /* the previous flow may already be gone */ }
    setBusy('');
    setInfo('');
  };

  const startBrowserLogin = async () => {
    setError('');
    setInfo('');
    if (!(await ensureServer())) return;
    setBusy('browser');
    setInfo('已在浏览器打开登录页，完成授权后会自动返回。');
    try {
      const settings = await invoke<GateSettings>('app_settings_get');
      const deviceLabel = settings.devify?.device_label?.trim() || 'AIMyChats';
      const url = await invoke<string>('devify_auth_start', { deviceLabel });
      await invoke('devify_auth_open', { url });
    } catch (reason) {
      await invoke('devify_auth_cancel').catch(() => {});
      setBusy('');
      setInfo('');
      setError(String(reason));
    }
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
    if (conflict.current) { setServerStatus('设置已变化，请重新打开页面并确认账号和服务地址。'); return; }
    const value = serverDraft.trim().replace(/\/+$/, '');
    if (!value) {
      setServerStatus('请填写服务端地址。');
      return;
    }
    const invalid = serverError(value);
    if (invalid) {
      setServerStatus(invalid);
      return;
    }
    if (!(await persistServer(value))) return;
    setServerOpen(false);
    setServerStatus('服务地址已保存。');
  };

  return <div className="auth-gate">
    <div className="auth-gate-card">
      <span className="auth-gate-mark"><img src={appIcon} alt=""/></span>
      <h1>需要登录</h1>
      <p>登录后才能连接聊天来源、备份和使用智能分析。</p>
      {notice && <p className="auth-gate-error" role="alert">{notice}</p>}
      {busy === 'browser'
        ? <div className="auth-gate-waiting" role="status"><span className="auth-gate-pulse"/><span>正在等待浏览器完成授权…</span></div>
        : <button type="button" className="auth-gate-primary" disabled={!settingsLoaded || serverSaving} onClick={() => {
            if (!settingsLoaded) return;
            if (!baseUrl.trim()) {
              setServerOpen(true);
              setServerStatus('请先填写服务端地址，再继续登录。');
              return;
            }
            setPopup(true); setError(''); setInfo('');
          }}>登录…</button>}
      {busy === 'browser' && <button type="button" className="auth-gate-secondary" onClick={() => void cancelBrowserLogin()}>取消</button>}
      {error && <p className="auth-gate-error" role="alert">{error}</p>}
      <div className="auth-gate-server">
        {serverOpen || !baseUrl
          ? <>
              <label>{baseUrl ? '服务端地址' : '服务端地址（首次使用请填写）'}
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
              {!baseUrl && <p className="auth-gate-hint">服务端不随客户端分发，请填写你们部署的服务端地址。</p>}
              <div className="auth-gate-server-actions">
                <button type="button" disabled={!settingsLoaded || serverSaving} onClick={() => void saveServer()}>保存</button>
                {baseUrl && <button type="button" onClick={() => { setServerOpen(false); draftRef.current = baseUrl; setServerDraft(baseUrl); if (!conflict.current) setServerStatus(''); }}>取消</button>}
              </div>
            </>
          : <button type="button" className="auth-gate-server-toggle" onClick={() => { setServerOpen(true); draftRef.current = baseUrl; setServerDraft(baseUrl); if (!conflict.current) setServerStatus(''); }}>服务地址 · {displayBaseUrl(baseUrl).replace(/^https?:\/\//, '').replace(/\/+$/, '')}</button>}
      </div>
      {serverStatus && <p className="auth-gate-hint" role="status">{serverStatus}</p>}
    </div>

    {popup && <div className="auth-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget && busy !== 'browser') { setPopup(false); setError(''); setInfo(''); } }}>
      <section className="auth-modal" role="dialog" aria-modal="true" aria-labelledby="auth-modal-title">
        <header className="auth-modal-head">
          <span id="auth-modal-title">登录 Aimychats</span>
          <button type="button" aria-label="关闭登录" disabled={busy === 'browser'} onClick={() => { setPopup(false); setError(''); setInfo(''); }}>×</button>
        </header>
        <div className="auth-modal-body">
          <div className="auth-tabs" role="tablist" aria-label="登录方式">
            {([['login', '登录'], ['register', '注册']] as Array<[Tab, string]>).map(([key, label]) => <button
              key={key} type="button" role="tab" aria-selected={tab === key} className={tab === key ? 'active' : ''}
              onClick={() => { setTab(key); setError(''); setInfo(''); }}
            >{label}</button>)}
          </div>

          {tab === 'login' && <>
            <button type="button" className="auth-alt" disabled={busy !== ''} onClick={() => void startBrowserLogin()}>
              <span className="auth-alt-mark">↗</span>通过浏览器登录服务端
            </button>
            <p className="auth-alt-help">沿用服务端授权回调：浏览器完成登录后会自动回到客户端，无需在本地输入密码。</p>
            <div className="auth-divider"><span>或使用 AI 邮箱账号</span></div>
            <label className="auth-field">AI 邮箱用户名
              <span className="auth-input-row">
                <input value={username} autoFocus placeholder="用户名" autoComplete="username" onChange={event => { setUsername(event.target.value.trim()); setError(''); }}/>
                <span className="auth-suffix">@aimychats.com</span>
              </span>
            </label>
            <label className="auth-field">密码
              <span className="auth-input-row">
                <input type={showPassword ? 'text' : 'password'} value={password} placeholder="密码" autoComplete="current-password" onChange={event => { setPassword(event.target.value); setError(''); }}/>
                <button type="button" className="auth-show" onClick={() => setShowPassword(value => !value)}>{showPassword ? '隐藏' : '显示'}</button>
              </span>
            </label>
            <div className="auth-forgot"><button type="button" onClick={() => void openExternal('/login')}>忘记密码？</button></div>
            {error && <p className="auth-error" role="alert">{error}</p>}
            {info && <p className="auth-info" role="status">{info}</p>}
            {busy === 'browser' && <div className="auth-waiting"><span className="auth-gate-pulse"/><span>正在等待浏览器完成授权…</span><button type="button" onClick={() => void cancelBrowserLogin()}>取消</button></div>}
            <button type="button" className="auth-submit" disabled={busy !== ''} onClick={() => void submitPassword()}>{busy === 'password' ? '正在登录…' : '登录'}</button>
            <p className="auth-switch">还没有账号？<button type="button" onClick={() => { setTab('register'); setError(''); setInfo(''); }}>注册</button></p>
          </>}

          {tab === 'register' && <>
            <div className="auth-register">
              <strong>注册需要先验证邮箱</strong>
              <p>在浏览器中创建 Aimychats 账号：填写邮箱并完成验证后，设置 AI 邮箱用户名和密码，再回到客户端登录。</p>
              <button type="button" className="auth-submit" onClick={() => void openExternal('/register')}>在浏览器中注册</button>
            </div>
            {error && <p className="auth-error" role="alert">{error}</p>}
            <p className="auth-switch">已有账号？<button type="button" onClick={() => { setTab('login'); setError(''); setInfo(''); }}>登录</button></p>
          </>}
        </div>
      </section>
    </div>}
  </div>;
}
