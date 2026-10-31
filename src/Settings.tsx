import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';

type SessionItem = {
  username: string;
  display: string;
  last_time: number | null;
  is_group: boolean;
  member_count: number | null;
  preview: string | null;
  unread: number | null;
};

type Account = { id: string; modified_at?: number | null };
type ScanResult = { accounts: Account[] };
type DevifySpace = { id: number; name: string; kind: string; status: string; role?: string; is_global_admin: boolean; admin_takeover_active: boolean };

/** The scheduler stores a status code, not a sentence; the page shows the sentence. */
const DAILY_STATUS_LABEL: Record<string, string> = {
  sent: '已投递',
  no_messages: '当天无新消息',
  busy: '账号正忙，本次跳过',
  error: '未完成',
};

/** Same rule as the chat page: the remembered account wins, otherwise the one WeChat is
 *  still writing to (the signed-in account), otherwise the only one found. */
function currentAccount(accounts: Account[], remembered: string): Account | null {
  if (!accounts.length) return null;
  const known = remembered ? accounts.find(account => account.id === remembered) : undefined;
  if (known) return known;
  return [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
}

export type AppSettings = {
  active_account_id: string;
  chat: {
    allowed_usernames: Record<string, string[]>;
    sync_interval_seconds: number;
  };
  devify: { base_url: string; device_label: string; active_space_id: number | null; recipient: string; subject: string };
  upload_queue?: Array<{ id: string; space_id: number; batch_id?: number | null; status: string; message: string; updated_at: string }>;
  automation: {
    enabled: boolean;
    daily_summary_enabled: boolean;
    daily_time: string;
    last_daily_run: string | null;
    last_daily_status: string | null;
    last_daily_error: string | null;
  };
  agent: {
    provider_id: string;
    command: string;
    model: string;
    timeout_seconds: number;
    instructions: string;
    use_for_daily: boolean;
  };
};

type MailStatus = {
  configured: boolean;
  host: string | null;
  port: number | null;
  from: string | null;
  missing: string[];
};

export function defaultSettings(): AppSettings {
  return {
    active_account_id: '',
    chat: { allowed_usernames: {}, sync_interval_seconds: 30 },
    devify: { base_url: 'https://app.aimychats.com', device_label: 'AImyChats', active_space_id: null, recipient: '', subject: '微信工作日报' },
    automation: {
      enabled: false,
      daily_summary_enabled: true,
      daily_time: '18:30',
      last_daily_run: null,
      last_daily_status: null,
      last_daily_error: null,
    },
    agent: {
      provider_id: '',
      command: '',
      model: '',
      timeout_seconds: 180,
      instructions: '',
      use_for_daily: true,
    },
  };
}

function accountLabel(account: Account): string {
  return account.id.length > 38 ? `${account.id.slice(0, 35)}…` : account.id;
}

export function mergeSettings(stored: AppSettings): AppSettings {
  const defaults = defaultSettings();
  return {
    ...defaults,
    ...stored,
    chat: { ...defaults.chat, ...stored.chat },
    devify: { ...defaults.devify, ...stored.devify },
    automation: { ...defaults.automation, ...stored.automation },
    agent: { ...defaults.agent, ...stored.agent },
  };
}

export function WhitelistSettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState('');
  const [catalog, setCatalog] = useState<SessionItem[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [kind, setKind] = useState<'all' | 'group' | 'direct'>('all');
  const [search, setSearch] = useState('');
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');

  const allowed = useMemo(
    () => new Set(settings.chat.allowed_usernames[accountId] ?? []),
    [settings.chat.allowed_usernames, accountId],
  );

  const matchedCatalog = useMemo(() => {
    const keyword = search.trim().toLowerCase();
    if (!keyword) return catalog;
    return catalog.filter(session =>
      session.display.toLowerCase().includes(keyword) || session.username.toLowerCase().includes(keyword),
    );
  }, [catalog, search]);
  const groupCount = useMemo(() => matchedCatalog.filter(session => session.is_group).length, [matchedCatalog]);
  // Official accounts / service sessions are filtered out of the catalog, so a whitelist
  // saved before that filter existed can still hold them. Count only what the user can see,
  // and drop the rest when saving so the stored list cannot go stale.
  const catalogUsernames = useMemo(() => new Set(catalog.map(session => session.username)), [catalog]);
  const allowedCount = useMemo(() => {
    if (!catalog.length) return allowed.size;
    let count = 0;
    allowed.forEach(username => {
      if (catalogUsernames.has(username)) count += 1;
    });
    return count;
  }, [allowed, catalog.length, catalogUsernames]);
  const visibleCatalog = useMemo(
    () => (kind === 'all'
      ? matchedCatalog
      : matchedCatalog.filter(session => (kind === 'group' ? session.is_group : !session.is_group))),
    [matchedCatalog, kind],
  );

  useEffect(() => {
    void (async () => {
      try {
        const [stored, scan] = await Promise.all([
          invoke<AppSettings>('app_settings_get'),
          invoke<ScanResult>('wechat_scan'),
        ]);
        const next = mergeSettings(stored);
        const list = scan.accounts ?? [];
        setSettings(next);
        setAccounts(list);
        setAccountId(currentAccount(list, next.active_account_id)?.id ?? '');
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

  useEffect(() => {
    if (!accountId) return;
    setCatalogLoading(true);
    setError('');
    void invoke<SessionItem[]>('wechat_session_catalog', { accountId })
      .then(setCatalog)
      .catch(e => {
        setCatalog([]);
        setError(`无法读取该账号的会话候选：${String(e)}`);
      })
      .finally(() => setCatalogLoading(false));
  }, [accountId]);

  const setVisibleSelection = (selected: boolean) => {
    const next = new Set(allowed);
    visibleCatalog.forEach(session => selected ? next.add(session.username) : next.delete(session.username));
    setSettings(current => ({ ...current, chat: { ...current.chat, allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] } } }));
  };

  const toggleUsername = (username: string) => {
    const next = new Set(allowed);
    if (next.has(username)) next.delete(username);
    else next.add(username);
    setSettings(current => ({
      ...current,
      chat: {
        ...current.chat,
        allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] },
      },
    }));
  };

  const save = async () => {
    setSaving(true);
    setError('');
    setNotice('');
    try {
      const keep = catalog.length
        ? (settings.chat.allowed_usernames[accountId] ?? []).filter(username => catalogUsernames.has(username))
        : settings.chat.allowed_usernames[accountId];
      const saved = await invoke<AppSettings>('app_settings_save', {
        value: {
          ...settings,
          active_account_id: accountId,
          chat: {
            ...settings.chat,
            allowed_usernames: { ...settings.chat.allowed_usernames, [accountId]: keep ?? [] },
          },
        },
      });
      setSettings(saved);
      setNotice('微信导入范围已保存，重新打开聊天记录页面后立即生效。');
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return <div className="settings-page imports-page">
    <header className="imports-page-head">
      <div><p className="imports-kicker">IMPORT MANAGEMENT</p><h2>导入管理</h2><p className="help">按连接器选择要导入的来源和会话。当前客户端先支持微信；其他来源接入后会在这里分别配置。</p></div>
      <span className="imports-count">{accountId ? `微信 · 已选 ${allowedCount} 个会话` : '尚未连接微信'}</span>
    </header>
    <nav className="imports-platforms" aria-label="导入来源"><button type="button" className="active"><span className="imports-platform-mark green">微</span><span><strong>微信</strong><small>本机数据库 · 只读</small></span></button><button type="button" disabled><span className="imports-platform-mark teal">W</span><span><strong>WhatsApp</strong><small>尚未接入</small></span></button><button type="button" disabled><span className="imports-platform-mark blue">@</span><span><strong>邮件</strong><small>尚未接入</small></span></button></nav>
    <section className="panel imports-panel">
      {/* One WeChat account is the normal case, so the page just says which one it is
          using; the picker only appears when this Mac really has several. */}
      <div className="settings-account">
        <span>当前账号</span>
        {accounts.length > 1 ? (
          <select value={accountId} onChange={event => setAccountId(event.target.value)}>
            {accounts.map(account => <option key={account.id} value={account.id}>{accountLabel(account)}</option>)}
          </select>
        ) : (
          <strong className="mono">{accountId || '未检测到微信账号'}</strong>
        )}
        {accountId && accounts.length <= 1 && <small>自动使用本机微信正在登录的账号</small>}
      </div>
      <div className="settings-subheading"><strong>导入范围</strong><span>{accountId ? `已选 ${allowedCount} 个` : '未检测到微信账号'}</span></div>
      <p className="help">群聊和单聊可以混合选择。未勾选任何会话时，聊天记录页面不会展示全部会话作为兜底。</p>
      {catalog.length > 0 && <div className="settings-toolbar">
        <div className="tabs" role="tablist" aria-label="按会话类型筛选">
          <button type="button" role="tab" aria-selected={kind === 'all'} className={kind === 'all' ? 'on' : ''} onClick={() => setKind('all')}>全部 {matchedCatalog.length}</button>
          <button type="button" role="tab" aria-selected={kind === 'group'} className={kind === 'group' ? 'on' : ''} onClick={() => setKind('group')}>群聊 {groupCount}</button>
          <button type="button" role="tab" aria-selected={kind === 'direct'} className={kind === 'direct' ? 'on' : ''} onClick={() => setKind('direct')}>单聊 {matchedCatalog.length - groupCount}</button>
        </div>
        <div className="settings-search">
          <input
            type="text"
            value={search}
            onChange={event => setSearch(event.target.value)}
            placeholder="搜索会话名或 wxid"
            spellCheck={false}
          />
          {search && <button type="button" className="secondary" onClick={() => setSearch('')}>清空</button>}
        </div>
        <div className="imports-bulk-actions"><span>批量设置当前结果</span><button type="button" className="secondary" onClick={() => setVisibleSelection(true)} disabled={!visibleCatalog.length}>全部导入</button><button type="button" className="secondary" onClick={() => setVisibleSelection(false)} disabled={!visibleCatalog.length}>全部忽略</button></div>
      </div>}
      {!accountId && <p className="settings-empty">未发现微信账号。请先在本地数据页面扫描并解密。</p>}
      {accountId && catalogLoading && <p className="settings-empty">正在加载会话候选…</p>}
      {accountId && !catalogLoading && !catalog.length && <p className="settings-empty">没有可配置的会话。请先完成该账号的解密。</p>}
      {catalog.length > 0 && search.trim() && !matchedCatalog.length && <p className="settings-empty">没有匹配「{search.trim()}」的会话。</p>}
      {matchedCatalog.length > 0 && !visibleCatalog.length && <p className="settings-empty">{kind === 'group' ? '没有匹配的群聊。' : '没有匹配的单聊。'}</p>}
      {visibleCatalog.length > 0 && <div className="settings-session-list">
        {visibleCatalog.map(session => <label key={session.username} className="settings-session">
          <input type="checkbox" checked={allowed.has(session.username)} onChange={() => toggleUsername(session.username)}/>
          <span><strong>{session.display}</strong><small>{session.is_group ? '群聊' : '单聊'} · {session.username}</small></span>
        </label>)}
      </div>}
    </section>

    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存微信导入范围'}</button>
    </div>
  </div>;
}

export function StrategySettings({ title = '自动化策略', description = '集中策略负责准实时同步和每日总结；聊天范围由“聊天白名单”页面单独管理。默认不自动发送。' }: { title?: string; description?: string } = {}) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(stored => setSettings(mergeSettings(stored)))
      .catch(e => setError(String(e)));
  }, []);

  const save = async () => {
    setSaving(true);
    setError('');
    setNotice('');
    try {
      await invoke('app_settings_save', { value: settings });
      setNotice('自动化策略已保存。');
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return <div className="settings-page">
    <header className="page-head">
      <h2>{title}</h2>
      <p className="help">{description}</p>
    </header>
    <section className="panel">
      <label className="confirm"><input type="checkbox" checked={settings.automation.enabled} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, enabled: event.target.checked } }))}/><span>启用自动化调度</span></label>
      <div className="grid">
        <label>同步间隔（秒）
          <input type="number" min={1} max={3600} value={settings.chat.sync_interval_seconds} onChange={event => setSettings(current => ({ ...current, chat: { ...current.chat, sync_interval_seconds: Number(event.target.value) || 1 } }))}/>
        </label>
        <label>日报时间
          <input type="time" value={settings.automation.daily_time} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, daily_time: event.target.value } }))}/>
        </label>
        <label>每日日报
          <select value={settings.automation.daily_summary_enabled ? 'on' : 'off'} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, daily_summary_enabled: event.target.value === 'on' } }))}>
            <option value="on">生成</option>
            <option value="off">暂不生成</option>
          </select>
        </label>
      </div>
      <p className="help">在「Agent 集成」页面选择本地 Agent 并允许用于日报后，每日日报会改用该 Agent 生成；失败时自动回退规则整理，原因记录在最近执行状态里。应用关闭时不会执行后台任务，托盘和开机启动将在下一步接入。</p>
      {settings.automation.last_daily_run && <p className="muted">最近执行：{settings.automation.last_daily_run} · {DAILY_STATUS_LABEL[settings.automation.last_daily_status ?? ''] ?? '未知'}</p>}
      {settings.automation.last_daily_error && <div className="config warn">{settings.automation.last_daily_error}</div>}
    </section>
    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存自动化策略'}</button>
    </div>
  </div>;
}

export function ServerAnalysisSettings() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [error, setError] = useState('');
  useEffect(() => { void invoke<AppSettings>('app_settings_get').then(setSettings).catch(reason => setError(String(reason))); }, []);
  const connected = Boolean(settings?.devify.active_space_id);
  return <div className="settings-page analysis-page">
    <header className="analysis-page-head"><div><p className="analysis-kicker">DEVIFY · SERVER ANALYSIS</p><h2>智能分析</h2><p className="help">事件、主题和报告由 Devify 服务端生成，桌面端负责提交数据并同步结果，不在本地伪造分析内容。</p></div><span className={connected ? 'analysis-connection on' : 'analysis-connection'}>{connected ? '服务端已连接' : '未选择空间'}</span></header>
    {error && <p className="error" role="alert">{error}</p>}
    <section className="analysis-cards"><article><span className="analysis-card-mark">事</span><div><h3>事件分析</h3><p>从已上传的聊天中识别待办、决定和进展，并在事件页面同步。</p><small>服务端生成 · 自动处理</small></div></article><article><span className="analysis-card-mark">题</span><div><h3>主题订阅</h3><p>按订阅周期回溯匹配的事件，生成主题结果和历史执行记录。</p><small>在「主题」页面管理</small></div></article><article><span className="analysis-card-mark">报</span><div><h3>日报 / 周报 / 月报</h3><p>基于服务端事件和已上传记录生成报告，结果在「报告」页面查看。</p><small>服务端生成 · 客户端同步</small></div></article></section>
    <section className="panel analysis-boundary"><strong>当前分析范围</strong><p>{connected ? '当前 Devify 空间中的已上传聊天、事件和笔记。未上传的本地记录不会进入服务端分析。' : '登录 Devify 并选择空间后，才能查看服务端分析结果。'}</p><div className="page-actions">{!connected && <span className="muted">请先完成 Devify 账户配置</span>}</div></section>
  </div>;
}

export function DevifySettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [status, setStatus] = useState<MailStatus | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [authStatus, setAuthStatus] = useState('');
  const [spaces, setSpaces] = useState<DevifySpace[]>([]);
  const [takeoverReason, setTakeoverReason] = useState('');
  const [spacesStatus, setSpacesStatus] = useState('');

  useEffect(() => {
    void Promise.all([
      invoke<AppSettings>('app_settings_get'),
      invoke<MailStatus>('mail_config_status'),
    ]).then(([stored, mail]) => {
      setSettings({ ...defaultSettings(), ...stored, devify: { ...defaultSettings().devify, ...stored.devify } });
      setStatus(mail);
    }).catch(e => setError(String(e)));
  }, []);

  useEffect(() => {
    let active = true;
    let cleanDeepLink: (() => void) | undefined;
    let cleanLoopback: (() => void) | undefined;
    const finishLogin = async (url: string) => {
      try {
        const result = await invoke<{ device_id: string }>('devify_auth_callback', { callbackUrl: url });
        if (!active) return;
        setAuthStatus(`Devify 已登录，设备 ${result.device_id}`);
        setSpacesStatus('正在读取可用空间…');
        const available = await invoke<DevifySpace[]>('devify_spaces');
        if (!active) return;
        setSpaces(available);
        setSpacesStatus(available.length ? `已读取 ${available.length} 个空间。` : '当前账号没有可用空间。');
        setError('');
      } catch (e) {
        if (active) setError(String(e));
      }
    };
    void onOpenUrl(urls => {
      for (const url of urls) {
        void finishLogin(url);
      }
    }).then(unlisten => { if (active) cleanDeepLink = unlisten; else unlisten(); }).catch(e => { if (active) setError(String(e)); });
    void listen<string>('devify-auth-loopback', event => {
      void finishLogin(event.payload);
    }).then(unlisten => { if (active) cleanLoopback = unlisten; else unlisten(); }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; cleanDeepLink?.(); cleanLoopback?.(); };
  }, []);

  const save = async () => {
    setSaving(true);
    setError('');
    setNotice('');
    try {
      const saved = await invoke<AppSettings>('app_settings_save', { value: settings });
      setSettings(mergeSettings(saved));
      setNotice('Devify 配置已保存。');
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const loginDevify = async () => {
    setError('');
    setAuthStatus('正在打开 Devify 登录…');
    try {
      const url = await invoke<string>('devify_auth_start', { deviceLabel: settings.devify.device_label });
      await invoke('devify_auth_open', { url });
    } catch (e) {
      setAuthStatus('');
      setError(String(e));
    }
  };

  const loadSpaces = async () => {
    setError('');
    setSpacesStatus('正在读取可用空间…');
    try {
      const available = await invoke<DevifySpace[]>('devify_spaces');
      setSpaces(available);
      setSpacesStatus(available.length ? `已读取 ${available.length} 个空间。` : '当前账号没有可用空间。');
    } catch (e) { setSpaces([]); setSpacesStatus('读取空间失败。'); setError(String(e)); }
  };

  const selectedSpace = spaces.find(space => space.id === settings.devify.active_space_id);
  const needsTakeover = Boolean(selectedSpace?.is_global_admin && !selectedSpace.admin_takeover_active);
  const takeoverSpace = async () => {
    if (!selectedSpace || !takeoverReason.trim()) return;
    try {
      await invoke('devify_space_takeover', { spaceId: selectedSpace.id, reason: takeoverReason.trim() });
      setTakeoverReason('');
      await loadSpaces();
    } catch (e) { setError(String(e)); }
  };

  return <div className="settings-page">
    <header className="page-head">
      <h2>Devify 接收配置</h2>
      <p className="help">客户端只需要配置 Devify 服务地址。设备身份、空间和上传目标由 Devify 登录后管理。</p>
    </header>
    <section className="panel">
      <div className="grid">
        <label>Devify 服务地址
          <input type="url" value={settings.devify.base_url} onChange={event => setSettings(current => ({ ...current, devify: { ...current.devify, base_url: event.target.value } }))}/>
        </label>
      </div>
      <div className="page-actions"><button className="secondary" onClick={() => void loginDevify()}>登录 Devify</button>{authStatus && <span className="muted">{authStatus}</span>}</div>
      <div className="grid">
        <label>上传目标空间
          <select value={settings.devify.active_space_id ?? ''} onChange={event => setSettings(current => ({ ...current, devify: { ...current.devify, active_space_id: event.target.value ? Number(event.target.value) : null } }))}>
            <option value="">尚未选择</option>
            {spaces.map(space => <option key={space.id} value={space.id}>{space.name} · {space.role || space.kind}</option>)}
          </select>
        </label>
      </div>
      {needsTakeover && <div className="config warn"><p>当前账号是全局管理员，访问空间业务数据前需要先接管空间并记录原因。</p><label>接管原因<input value={takeoverReason} maxLength={500} onChange={event => setTakeoverReason(event.target.value)} /></label><button type="button" className="secondary" disabled={!takeoverReason.trim()} onClick={() => void takeoverSpace()}>确认接管空间</button></div>}
      <div className="page-actions"><button className="secondary" onClick={() => void loadSpaces()}>读取可用空间</button></div>
      {!settings.devify.active_space_id && <p className="help">请先选择并保存上传目标空间。</p>}
      {spacesStatus && <div className="muted" role="status">{spacesStatus}</div>}
      <div className={`config ${status?.configured ? 'ok' : 'warn'}`}>
        {status === null ? '正在检查 SMTP 配置…' : status.configured ? `SMTP 已配置：${status.host}:${status.port}，发件人 ${status.from}` : `SMTP 未配置。缺少：${status.missing.join(', ')}`}
      </div>
      <p className="help">
        开发阶段通过 <code>DEVIFY_SMTP_HOST</code>、<code>DEVIFY_SMTP_PORT</code>、<code>DEVIFY_SMTP_USERNAME</code>、
        <code>DEVIFY_SMTP_PASSWORD</code> 和 <code>DEVIFY_MAIL_FROM</code> 配置 SMTP。正式版本再迁移到系统凭据存储。
      </p>
    </section>
    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存 Devify 配置'}</button>
    </div>
  </div>;
}
