import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import appIcon from './assets/app-icon.png';

type GateSettings = {
  devify?: { base_url?: string; device_label?: string };
  [key: string]: unknown;
};

type Tab = 'login' | 'register';

const DEFAULT_BASE_URL = 'https://app.aimychats.com';

export function LoginGate({ notice = '', onAuthenticated }: { notice?: string; onAuthenticated: () => void }) {
  const [popup, setPopup] = useState(false);
  const [tab, setTab] = useState<Tab>('login');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState<'' | 'password' | 'browser'>('');
  const [error, setError] = useState('');
  const [info, setInfo] = useState('');
  const [baseUrl, setBaseUrl] = useState(DEFAULT_BASE_URL);
  const [serverOpen, setServerOpen] = useState(false);
  const [serverDraft, setServerDraft] = useState(DEFAULT_BASE_URL);
  const [serverStatus, setServerStatus] = useState('');
  const completed = useRef(false);

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
        const value = stored.devify?.base_url?.trim() || DEFAULT_BASE_URL;
        setBaseUrl(value);
        setServerDraft(value);
      })
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!popup) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && busy !== 'browser') { setPopup(false); setError(''); setInfo(''); }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [popup, busy]);

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

  const submitPassword = () => {
    if (!idOk) { setError('请输入用户名（至少 3 位，可用字母、数字和 . _ -）'); return; }
    if (!password) { setError('请输入密码'); return; }
    void submitLogin();
  };

  const cancelBrowserLogin = async () => {
    try { await invoke('devify_auth_cancel'); } catch { /* the previous flow may already be gone */ }
    setBusy('');
    setInfo('');
  };

  const startBrowserLogin = async () => {
    setBusy('browser');
    setError('');
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
    try {
      await invoke('devify_auth_open', { url: `${baseUrl.replace(/\/+$/, '')}${path}` });
    } catch (reason) {
      setError(String(reason));
    }
  };

  const saveServer = async () => {
    const value = serverDraft.trim().replace(/\/+$/, '');
    try {
      const parsed = new URL(value);
      if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') throw new Error('invalid');
    } catch {
      setServerStatus('请输入完整的服务地址，例如 https://app.aimychats.com。');
      return;
    }
    try {
      const stored = await invoke<GateSettings>('app_settings_get');
      const next: GateSettings = { ...stored, devify: { ...stored.devify, base_url: value } };
      await invoke('app_settings_save', { value: next });
      setBaseUrl(value);
      setServerOpen(false);
      setServerStatus('服务地址已保存。');
      setError('');
    } catch (reason) {
      setServerStatus(String(reason));
    }
  };

  return <div className="auth-gate">
    <div className="auth-gate-card">
      <span className="auth-gate-mark"><img src={appIcon} alt=""/></span>
      <h1>需要登录</h1>
      <p>登录后才能连接聊天来源、备份和使用智能分析。</p>
      {notice && <p className="auth-gate-error" role="alert">{notice}</p>}
      {busy === 'browser'
        ? <div className="auth-gate-waiting" role="status"><span className="auth-gate-pulse"/><span>正在等待浏览器完成授权…</span></div>
        : <button type="button" className="auth-gate-primary" onClick={() => { setPopup(true); setError(''); setInfo(''); }}>登录…</button>}
      {busy === 'browser' && <button type="button" className="auth-gate-secondary" onClick={() => void cancelBrowserLogin()}>取消</button>}
      {error && <p className="auth-gate-error" role="alert">{error}</p>}
      <div className="auth-gate-server">
        {serverOpen
          ? <>
              <label>Devify 服务地址
                <input type="url" value={serverDraft} onChange={event => setServerDraft(event.target.value)} placeholder={DEFAULT_BASE_URL}/>
              </label>
              <div className="auth-gate-server-actions">
                <button type="button" onClick={() => void saveServer()}>保存</button>
                <button type="button" onClick={() => { setServerOpen(false); setServerDraft(baseUrl); }}>取消</button>
              </div>
            </>
          : <button type="button" className="auth-gate-server-toggle" onClick={() => { setServerOpen(true); setServerDraft(baseUrl); }}>服务地址 · {baseUrl.replace(/^https?:\/\//, '')}</button>}
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
              <span className="auth-alt-mark">↗</span>通过浏览器登录 Devify
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
            <button type="button" className="auth-submit" disabled={busy !== ''} onClick={submitPassword}>{busy === 'password' ? '正在登录…' : '登录'}</button>
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
