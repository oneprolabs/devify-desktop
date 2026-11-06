import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import { registerDevifyDevice } from './devifyDevice';

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

const SYNC_STATUS_LABEL: Record<string, string> = {
  success: '已完成',
  failed: '未完成',
  idle: '未运行',
  syncing: '同步中',
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
    initial_ranges: Record<string, Record<string, { since: number | null; until: number | null }>>;
    sync_interval_seconds: number;
  };
  connectors: { waitlist: string[]; wechat_enabled: boolean };
  devify: { base_url: string; device_label: string; device_id: string; platform: string; active_space_id: number | null; recipient: string; subject: string };
  upload_queue?: Array<{ id: string; space_id: number; batch_id?: number | null; status: string; message: string; updated_at: string }>;
  voice: {
    auto_transcribe: boolean;
    upload_transcripts: boolean;
  };
  sync: {
    enabled: boolean;
    cursors?: Record<string, Record<string, unknown>>;
    last_run_at: string | null;
    last_status: string | null;
    last_error: string | null;
    last_message_count: number;
  };
  backup: {
    mode: 'interval' | 'times' | 'manual';
    times: string[];
    window_enabled: boolean;
    window_mode: 'deny' | 'allow';
    window_start: string;
    window_end: string;
    /** 0 = 周一 … 6 = 周日. */
    days: number[];
    /** Only upload over Wi-Fi or a wired link, never a tethered phone. */
    wifi_only: boolean;
    /** text = no attachments, images = pictures, all = every attachment up to 25 MB. */
    attachments: 'text' | 'images' | 'all';
  };
  language: { ui: string; summary: string; ask: string };
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

export function defaultSettings(): AppSettings {
  return {
    active_account_id: '',
    chat: { allowed_usernames: {}, initial_ranges: {}, sync_interval_seconds: 1800 },
    connectors: { waitlist: [], wechat_enabled: true },
    devify: { base_url: '', device_label: 'AIMyChats', device_id: '', platform: '', active_space_id: null, recipient: '', subject: '微信工作日报' },
    voice: { auto_transcribe: true, upload_transcripts: true },
    sync: { enabled: false, last_run_at: null, last_status: null, last_error: null, last_message_count: 0 },
    language: { ui: '中文', summary: '跟随界面', ask: '跟随提问' },
    backup: {
      mode: 'interval',
      times: ['18:00'],
      window_enabled: false,
      window_mode: 'deny',
      window_start: '23:00',
      window_end: '07:00',
      days: [0, 1, 2, 3, 4, 5, 6],
      wifi_only: true,
      attachments: 'images',
    },
    automation: {
      enabled: false,
      daily_summary_enabled: true,
      daily_time: '08:30',
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
  const chat = { ...defaults.chat, ...stored.chat };
  // The stepper works in whole minutes/hours; a 30-second legacy value would be shown as
  // "1 分钟" while the real schedule stayed at 30 seconds. Snap it back to the default.
  if (!(chat.sync_interval_seconds >= 60)) chat.sync_interval_seconds = defaults.chat.sync_interval_seconds;
  return {
    ...defaults,
    ...stored,
    chat,
    connectors: { ...defaults.connectors, ...(stored.connectors ?? {}) },
    devify: { ...defaults.devify, ...stored.devify },
    voice: { ...defaults.voice, ...(stored.voice ?? {}) },
    sync: { ...defaults.sync, ...stored.sync },
    backup: { ...defaults.backup, ...(stored.backup ?? {}) },
    language: { ...defaults.language, ...(stored.language ?? {}) },
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
  const [open, setOpen] = useState(true);
  const [search, setSearch] = useState('');
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [dateDetails, setDateDetails] = useState<Record<string, Array<[string, number]>>>({});
  const [dateLoading, setDateLoading] = useState('');

  const dateText = (seconds: number | null | undefined, end = false) => {
    if (seconds == null) return '';
    const date = new Date(seconds * 1000);
    if (end) date.setDate(date.getDate() - 1);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
  };
  const setDate = (username: string, key: 'since' | 'until', text: string) => {
    const date = text ? new Date(`${text}T00:00:00`) : null;
    if (date && key === 'until') date.setDate(date.getDate() + 1);
    const range = settings.chat.initial_ranges[accountId]?.[username] ?? { since: null, until: null };
    setSettings(current => ({ ...current, chat: { ...current.chat, initial_ranges: { ...current.chat.initial_ranges, [accountId]: { ...current.chat.initial_ranges[accountId], [username]: { ...range, [key]: date ? Math.floor(date.getTime() / 1000) : null } } } } }));
  };
  const loadDates = async (username: string) => {
    setDateLoading(username); setError('');
    try { const dates = await invoke<Array<[string, number]>>('wechat_catalog_dates', { accountId, username }); setDateDetails(current => ({ ...current, [username]: dates })); }
    catch (reason) { setError(`读取有记录的日期失败：${String(reason)}`); }
    finally { setDateLoading(''); }
  };

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
      if (Object.values(settings.chat.initial_ranges[accountId] ?? {}).some(range => range.since != null && range.until != null && range.since >= range.until)) throw new Error('历史记录结束日期不能早于开始日期。');
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
    <header className="page-head">
      <h2>导入管理</h2>
      <p className="help">每个平台单独选择要备份的群和会话。会话很多时，用筛选和批量操作，不必逐个点。连接向导里也可以设置。</p>
    </header>

    <section className={`plan-plat${open ? ' open' : ''}`}>
      <div className="plan-plat-head">
        <span className="plan-mark">微</span>
        <button type="button" className="plan-plat-title" aria-expanded={open} onClick={() => setOpen(!open)}>
          <strong>微信</strong>
          <small>{accountId ? `已选 ${allowedCount} 个会话 · 本机数据库 · 只读` : '尚未检测到账号'}<i>{open ? '收起' : '编辑'} →</i></small>
        </button>
      </div>
      {open && <div className="plan-rows">
        <div className="plan-row">
          <div className="plan-row-label">
            <span>当前账号</span>
            <small>{accounts.length > 1 ? '检测到多个微信账号，选择要导入的那个' : '自动使用本机微信正在登录的账号'}</small>
          </div>
          <div className="plan-row-control">
            {accounts.length > 1
              ? <select className="plan-select" value={accountId} aria-label="微信账号" onChange={event => setAccountId(event.target.value)}>
                {accounts.map(account => <option key={account.id} value={account.id}>{accountLabel(account)}</option>)}
              </select>
              : <span className="plan-kv mono">{accountId || '未检测到微信账号'}</span>}
          </div>
        </div>

        {catalog.length > 0 && <>
          <div className="plan-row">
            <div className="plan-row-label">
              <span>筛选会话</span>
              <small>群聊和单聊可以混合选择；未勾选任何会话时，聊天页不会兜底展示全部会话</small>
            </div>
            <div className="plan-row-control">
              <div className="plan-pills" role="group" aria-label="按会话类型筛选">
                <button type="button" aria-pressed={kind === 'all'} className={kind === 'all' ? 'on' : ''} onClick={() => setKind('all')}>全部 {matchedCatalog.length}</button>
                <button type="button" aria-pressed={kind === 'group'} className={kind === 'group' ? 'on' : ''} onClick={() => setKind('group')}>群聊 {groupCount}</button>
                <button type="button" aria-pressed={kind === 'direct'} className={kind === 'direct' ? 'on' : ''} onClick={() => setKind('direct')}>单聊 {matchedCatalog.length - groupCount}</button>
              </div>
              <input className="plan-input" type="text" value={search} spellCheck={false} placeholder="搜索会话名或 wxid" aria-label="搜索会话" onChange={event => setSearch(event.target.value)}/>
              {search && <button type="button" className="plan-button" onClick={() => setSearch('')}>清空</button>}
            </div>
          </div>

          <div className="plan-row">
            <div className="plan-row-label">
              <span>批量设置</span>
              <small>只作用于当前筛选结果，共 {visibleCatalog.length} 个会话</small>
            </div>
            <div className="plan-row-control">
              <button type="button" className="plan-button" disabled={!visibleCatalog.length} onClick={() => setVisibleSelection(true)}>全部导入</button>
              <button type="button" className="plan-button" disabled={!visibleCatalog.length} onClick={() => setVisibleSelection(false)}>全部忽略</button>
            </div>
          </div>

          <div className="imports-list">
            {visibleCatalog.map(session => <div key={session.username}>
              <label className="imports-session"><input type="checkbox" checked={allowed.has(session.username)} onChange={() => toggleUsername(session.username)}/><span><strong>{session.display}</strong><small>{session.is_group ? '群聊' : '单聊'} · {session.username}</small></span></label>
              {allowed.has(session.username) && !!settings.sync.cursors?.[accountId]?.[session.username] && <p style={{ padding: '0 24px 12px' }}>此会话已有同步进度，首次历史范围不再生效。</p>}
              {allowed.has(session.username) && !settings.sync.cursors?.[accountId]?.[session.username] && <div className="plan-row-control" style={{ padding: '0 24px 16px', flexWrap: 'wrap' }}>
                <span>首次上传历史范围</span>
                <label>开始 <input type="date" value={dateText(settings.chat.initial_ranges[accountId]?.[session.username]?.since)} onChange={e => setDate(session.username, 'since', e.target.value)}/></label>
                <label>结束 <input type="date" value={dateText(settings.chat.initial_ranges[accountId]?.[session.username]?.until, true)} onChange={e => setDate(session.username, 'until', e.target.value)}/></label>
                <button type="button" className="plan-button" disabled={!!dateLoading} onClick={() => void loadDates(session.username)}>{dateLoading === session.username ? '正在读取…' : '查看有记录的日期'}</button>
                {dateDetails[session.username] && <details><summary>{dateDetails[session.username].length} 天有记录</summary><p>{dateDetails[session.username].map(([day, count]) => `${day}（${count} 条）`).join('、') || '没有可用记录'}</p></details>}
              </div>}
            </div>)}
          </div>
        </>}

        {!accountId && <p className="plan-empty">未发现微信账号。请先在「连接器」完成扫描并解密。</p>}
        {accountId && catalogLoading && <p className="plan-empty">正在加载会话候选…</p>}
        {accountId && !catalogLoading && !catalog.length && <p className="plan-empty">没有可配置的会话。请先完成该账号的解密。</p>}
        {catalog.length > 0 && search.trim() && !matchedCatalog.length && <p className="plan-empty">没有匹配「{search.trim()}」的会话。</p>}
        {matchedCatalog.length > 0 && !visibleCatalog.length && <p className="plan-empty">{kind === 'group' ? '没有匹配的群聊。' : '没有匹配的单聊。'}</p>}
      </div>}
    </section>

    <div className="plan-off-list">
      {[{ mark: 'W', name: 'WhatsApp' }, { mark: '@', name: '邮件' }].map(item => <div className="plan-off" key={item.name}>
        <span className="plan-mark dim alt">{item.mark}</span>
        <div><strong>{item.name}</strong><small>连接后可单独设置导入范围</small></div>
        <button type="button" className="ghost" disabled>尚未接入</button>
      </div>)}
    </div>

    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存导入范围'}</button>
    </div>
  </div>;
}

export function StrategySettings({ title = '本地自动化', description = '每日总结在本机生成后投递到 Devify。上传频率、上传时段由「备份周期」页面单独配置。' }: { title?: string; description?: string } = {}) {
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
      setNotice('本地自动化设置已保存。');
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
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存本地自动化'}</button>
    </div>
  </div>;
}

export function DevifySettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [authStatus, setAuthStatus] = useState('');
  const [authenticated, setAuthenticated] = useState(false);
  const [spaces, setSpaces] = useState<DevifySpace[]>([]);
  const [takeoverReason, setTakeoverReason] = useState('');
  const [spacesStatus, setSpacesStatus] = useState('');

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(stored => setSettings({ ...defaultSettings(), ...stored, devify: { ...defaultSettings().devify, ...stored.devify } }))
      .catch(e => setError(String(e)));
  }, []);

  useEffect(() => {
    let active = true;
    void invoke<{ authenticated: boolean }>('devify_auth_status')
      .then(status => {
        if (!active) return;
        setAuthenticated(status.authenticated);
        if (!status.authenticated) return;
        void (async () => {
          await registerDevifyDevice();
          if (!active) return;
          await invoke<number | null>('devify_space_ensure').catch(() => null);
          if (!active) return;
          setSettings(mergeSettings(await invoke<AppSettings>('app_settings_get')));
          await loadSpaces();
        })();
      })
      .catch(() => {});
    return () => { active = false; };
  }, []);

  useEffect(() => {
    let active = true;
    let cleanDeepLink: (() => void) | undefined;
    let cleanLoopback: (() => void) | undefined;
    const finishLogin = async (url: string) => {
      try {
        const result = await invoke<{ device_id: string }>('devify_auth_callback', { callbackUrl: url });
        if (!active) return;
        setAuthenticated(true);
        await invoke<number | null>('devify_space_ensure').catch(() => null);
        setSettings(mergeSettings(await invoke<AppSettings>('app_settings_get')));
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

  const logoutDevify = async () => {
    setError('');
    try {
      await invoke('devify_logout');
      setAuthenticated(false);
      setSpaces([]);
      setSpacesStatus('');
      setAuthStatus('已退出登录。');
      window.dispatchEvent(new CustomEvent('aimychats:auth-changed', { detail: { authenticated: false } }));
    } catch (e) { setError(String(e)); }
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
  const activeSpace = selectedSpace;
  const needsTakeover = Boolean(selectedSpace?.is_global_admin && !selectedSpace.admin_takeover_active);
  const takeoverSpace = async () => {
    if (!selectedSpace || !takeoverReason.trim()) return;
    try {
      await invoke('devify_space_takeover', { spaceId: selectedSpace.id, reason: takeoverReason.trim() });
      setTakeoverReason('');
      await loadSpaces();
    } catch (e) { setError(String(e)); }
  };

  /** 账户页的控件直接落库：文本字段在失焦时提交，选择类立刻提交。 */
  const persist = (next: AppSettings) => {
    setSettings(next);
    void invoke<AppSettings>('app_settings_save', { value: next })
      .then(saved => { setSettings(mergeSettings(saved)); setError(''); })
      .catch(e => setError(`保存失败：${String(e)}`));
  };
  const setDevify = (patch: Partial<AppSettings['devify']>) => ({ ...settings, devify: { ...settings.devify, ...patch } });

  return <div className="settings-page account-settings">
    <header className="page-head">
      <h2>账户</h2>
      <p className="help">管理当前登录的 AIMyChats 账号。登录后默认使用个人空间，聊天记录与报告都写入这里。</p>
    </header>

    <div className="plan-items">
      <div className="plan-row">
        <div className="plan-row-label">
          <span>当前账号</span>
          <small>{authenticated ? (authStatus || 'Devify 已登录，设备身份由服务端签发') : '未登录 Devify，上传与报告同步都不可用'}</small>
        </div>
        <div className="plan-row-control">
          {authenticated
            ? <button type="button" className="plan-button" onClick={() => void logoutDevify()}>退出登录</button>
            : <button type="button" className="plan-button primary" onClick={() => void loginDevify()}>登录 Devify</button>}
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>Devify 服务地址</span>
          <small>自托管实例的根地址，上传与同步都发往这里</small>
        </div>
        <div className="plan-row-control">
          <input
            className="plan-input"
            type="url"
            aria-label="Devify 服务地址"
            placeholder="https://devify.example.com"
            value={settings.devify.base_url}
            onChange={event => setSettings(setDevify({ base_url: event.target.value }))}
            onBlur={() => persist(settings)}
          />
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>当前设备</span>
          <small>平台与设备标识会随上传发送到服务端，并显示在网页的「设备与设置」</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">
            {settings.devify.device_label || '未命名设备'} · {settings.devify.platform || '自动识别'}
            {settings.devify.device_id ? ` · ${settings.devify.device_id.slice(0, 8)}` : ' · 未登记'}
          </span>
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>当前空间</span>
          <small>{activeSpace ? '登录后自动使用个人空间，聊天记录、事件与报告都写入这里' : (spacesStatus || '登录后自动选择个人空间')}</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{activeSpace ? `${activeSpace.name} · ${activeSpace.role || activeSpace.kind}` : (authenticated ? '正在读取空间…' : '未登录')}</span>
        </div>
      </div>

    </div>

    {needsTakeover && <section className="account-takeover">
      <strong>当前账号是全局管理员</strong>
      <p>访问空间业务数据前需要先接管空间并记录原因。</p>
      <div className="account-takeover-actions">
        <input value={takeoverReason} maxLength={500} placeholder="接管原因" onChange={event => setTakeoverReason(event.target.value)}/>
        <button type="button" className="plan-button" disabled={!takeoverReason.trim()} onClick={() => void takeoverSpace()}>确认接管空间</button>
      </div>
    </section>}

    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}
  </div>;
}
