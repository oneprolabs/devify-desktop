import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import { registerDevifyDevice } from './devifyDevice';
import { displayBaseUrl } from './serviceUrl.ts';

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
type WechatProfile = { nickname: string | null; alias: string | null; avatar: string | null };

function useWechatProfiles(accounts: Account[]) {
  const [profiles, setProfiles] = useState<Record<string, WechatProfile>>({});
  useEffect(() => {
    let active = true;
    setProfiles({});
    void Promise.all(accounts.map(async account => {
      const profile = await invoke<WechatProfile | null>('wechat_account_profile', { accountId: account.id }).catch(() => null);
      return [account.id, profile] as const;
    })).then(results => {
      if (active) setProfiles(Object.fromEntries(results.filter((item): item is readonly [string, WechatProfile] => item[1] !== null)));
    });
    return () => { active = false; };
  }, [accounts]);
  return profiles;
}

function WechatIdentity({ account, profile }: { account: Account; profile?: WechatProfile }) {
  return <><span className="wechat-account-avatar">{profile?.avatar ? <img src={profile.avatar} alt="" /> : '微'}</span><span className="wechat-account-details"><strong>{profile?.nickname || '微信账号'}</strong><small>{profile?.alias ? `微信号：${profile.alias}` : '微信号暂不可用'}</small><small>本机标识：{account.id}</small></span></>;
}
type DevifySpace = { id: number; name: string; kind: string; status: string; role?: string; is_global_admin: boolean; admin_takeover_active: boolean };

/** The scheduler stores a status code, not a sentence; the page shows the sentence. */
const DAILY_STATUS_LABEL: Record<string, string> = {
  scheduled: '服务端已计划',
  disabled: '已关闭',
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
  user_settings_revision: number;
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
    daily_topic_id?: number | null;
    daily_topic_space_id?: number | null;
    last_schedule_sync_at?: string | null;
    last_schedule_signature?: string | null;
    last_schedule_error?: string | null;
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
    user_settings_revision: 0,
    active_account_id: '',
    chat: { allowed_usernames: {}, initial_ranges: {}, sync_interval_seconds: 1800 },
    connectors: { waitlist: [], wechat_enabled: true },
    devify: { base_url: 'https://aimychats.com/', device_label: 'AIMyChats', device_id: '', platform: '', active_space_id: null, recipient: '', subject: '微信工作日报' },
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
      daily_topic_id: null,
      daily_topic_space_id: null,
      last_schedule_sync_at: null,
      last_schedule_signature: null,
      last_schedule_error: null,
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

export function requireSettingsRevision<T extends { user_settings_revision?: unknown }>(stored: T): T {
  if (!Number.isSafeInteger(stored?.user_settings_revision) || (stored.user_settings_revision as number) < 0) {
    throw new Error('设置版本缺失或无效，请更新应用并重新打开页面。');
  }
  return stored;
}

function settingsErrorMessage(reason: unknown): string {
  if (typeof reason === 'string') return reason;
  if (reason && typeof reason === 'object' && 'message' in reason) return String(reason.message);
  try { return JSON.stringify(reason) ?? String(reason); } catch { return String(reason); }
}

export function isSettingsConflict(reason: unknown): boolean {
  return settingsErrorMessage(reason).includes('SETTINGS_CONFLICT:');
}

export function settingsSaveError(reason: unknown): string {
  const message = settingsErrorMessage(reason);
  if (message.includes('SETTINGS_CONFLICT:')) {
    return `设置已在其他页面或账号/空间中变化，未保存。请重新打开页面，确认当前账号和空间后重新操作。${message.split('SETTINGS_CONFLICT:').slice(1).join('SETTINGS_CONFLICT:')}`;
  }
  return `保存失败：${message}。请检查当前账号和空间后重试。`;
}

/** Apply a confirmed response without discarding edits made after the request began. */
export function mergeSavedSettings(saved: AppSettings, submitted: AppSettings, draft: AppSettings): AppSettings {
  const merge = (confirmed: unknown, sent: unknown, current: unknown): unknown => {
    if (Object.is(current, sent)) return confirmed;
    if (current && sent && confirmed && typeof current === 'object' && typeof sent === 'object'
      && typeof confirmed === 'object' && !Array.isArray(current) && !Array.isArray(sent) && !Array.isArray(confirmed)) {
      const result: Record<string, unknown> = { ...confirmed as Record<string, unknown> };
      const before = sent as Record<string, unknown>;
      const now = current as Record<string, unknown>;
      for (const key of new Set([...Object.keys(before), ...Object.keys(now)])) {
        if (!(key in now)) delete result[key];
        else result[key] = merge(result[key], before[key], now[key]);
      }
      return result;
    }
    return current;
  };
  const result = merge(saved, submitted, draft) as AppSettings;
  // A revision is backend-owned, even if a stale render was used by an input handler.
  result.user_settings_revision = saved.user_settings_revision;
  return result;
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

export function WhitelistSettings({ onSaved }: { onSaved?: () => void } = {}) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [loaded, setLoaded] = useState(false);
  const saveLock = useRef(false);
  const confirmedRevision = useRef<number | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const profiles = useWechatProfiles(accounts);
  const [accountId, setAccountId] = useState('');
  const accountIdRef = useRef('');
  const [catalog, setCatalog] = useState<SessionItem[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [kind, setKind] = useState<'all' | 'group' | 'direct'>('all');
  const [search, setSearch] = useState('');
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [dateDetails, setDateDetails] = useState<Record<string, Array<[string, number]>>>({});
  const [dateLoading, setDateLoading] = useState('');
  const [focusedSession, setFocusedSession] = useState('');
  const [customRanges, setCustomRanges] = useState<Record<string, boolean>>({});

  const dateText = (seconds: number | null | undefined, end = false) => {
    if (seconds == null) return '';
    const date = new Date(seconds * 1000);
    if (end) date.setDate(date.getDate() - 1);
    return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
  };
  const setDate = (username: string, key: 'since' | 'until', text: string) => {
    const date = text ? new Date(`${text}T00:00:00`) : null;
    if (date && key === 'until') date.setDate(date.getDate() + 1);
    setSettings(current => {
      const range = current.chat.initial_ranges[accountId]?.[username] ?? { since: null, until: null };
      return { ...current, chat: { ...current.chat, initial_ranges: { ...current.chat.initial_ranges, [accountId]: { ...current.chat.initial_ranges[accountId], [username]: { ...range, [key]: date ? Math.floor(date.getTime() / 1000) : null } } } } };
    });
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
        const next = mergeSettings(requireSettingsRevision(stored));
        const list = scan.accounts ?? [];
        confirmedRevision.current = next.user_settings_revision;
        setSettings(next);
        setLoaded(true);
        setAccounts(list);
        const id = currentAccount(list, next.active_account_id)?.id ?? '';
        accountIdRef.current = id;
        setCatalogLoading(Boolean(id));
        setAccountId(id);
      } catch (e) {
        setError(String(e));
      }
    })();
  }, []);

  useEffect(() => {
    if (!accountId) return;
    let active = true;
    setCatalog([]);
    setCatalogLoading(true);
    setError('');
    void invoke<SessionItem[]>('wechat_session_catalog', { accountId })
      .then(items => { if (active && accountIdRef.current === accountId) setCatalog(items); })
      .catch(e => {
        if (!active || accountIdRef.current !== accountId) return;
        setCatalog([]);
        setError(`无法读取该账号的会话候选：${String(e)}`);
      })
      .finally(() => { if (active && accountIdRef.current === accountId) setCatalogLoading(false); });
    return () => { active = false; };
  }, [accountId]);

  const setVisibleSelection = (selected: boolean) => {
    setSettings(current => {
      const next = new Set(current.chat.allowed_usernames[accountId] ?? []);
      visibleCatalog.forEach(session => selected ? next.add(session.username) : next.delete(session.username));
      return { ...current, chat: { ...current.chat, allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] } } };
    });
  };

  const toggleUsername = (username: string) => {
    if (!allowed.has(username)) setFocusedSession(username);
    setSettings(current => {
      const next = new Set(current.chat.allowed_usernames[accountId] ?? []);
      if (next.has(username)) next.delete(username);
      else next.add(username);
      return { ...current, chat: { ...current.chat, allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] } } };
    });
  };

  const save = async () => {
    if (!loaded || !accountId || catalogLoading || confirmedRevision.current === null || saveLock.current) return;
    saveLock.current = true;
    setSaving(true);
    setSaveError('');
    setError('');
    setNotice('');
    try {
      const missingRange = (settings.chat.allowed_usernames[accountId] ?? []).some(username => {
        const range = settings.chat.initial_ranges[accountId]?.[username];
        return customRanges[`${accountId}\0${username}`] === true && range?.since == null && range?.until == null && !settings.sync.cursors?.[accountId]?.[username];
      });
      if (missingRange) throw new Error('请选择自定义历史范围的开始或结束日期，或勾选上传全部历史记录。');
      if (Object.values(settings.chat.initial_ranges[accountId] ?? {}).some(range => range.since != null && range.until != null && range.since >= range.until)) throw new Error('历史记录结束日期不能早于开始日期。');
      const keep = catalog.length
        ? (settings.chat.allowed_usernames[accountId] ?? []).filter(username => catalogUsernames.has(username))
        : settings.chat.allowed_usernames[accountId];
      const submitted: AppSettings = {
        ...settings,
        user_settings_revision: confirmedRevision.current,
        active_account_id: accountId,
        chat: {
          ...settings.chat,
          allowed_usernames: { ...settings.chat.allowed_usernames, [accountId]: keep ?? [] },
        },
      };
      const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: submitted })));
      confirmedRevision.current = saved.user_settings_revision;
      setSettings(current => mergeSavedSettings(saved, settings, current));
      setNotice('微信导入范围已保存，重新打开聊天记录页面后立即生效。');
      onSaved?.();
    } catch (e) {
      setSaveError(settingsSaveError(e));
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };

  const focused = visibleCatalog.find(session => session.username === focusedSession) ?? visibleCatalog[0];
  const focusedRange = focused ? settings.chat.initial_ranges[accountId]?.[focused.username] : undefined;
  const alreadySynced = focused && !!settings.sync.cursors?.[accountId]?.[focused.username];
  const customRangeKey = focused ? `${accountId}\0${focused.username}` : '';
  const isCustomRange = customRanges[customRangeKey] ?? (focusedRange?.since != null || focusedRange?.until != null);
  const setUploadAllHistory = (uploadAll: boolean) => {
    if (!focused) return;
    setCustomRanges(current => ({ ...current, [customRangeKey]: !uploadAll }));
    if (uploadAll) setSettings(current => ({ ...current, chat: { ...current.chat, initial_ranges: { ...current.chat.initial_ranges, [accountId]: { ...current.chat.initial_ranges[accountId], [focused.username]: { since: null, until: null } } } } }));
  };

  if (!loaded) return <div className="settings-page">{error ? <div className="error">设置加载失败：{error} 请重新打开页面。</div> : <p>正在加载设置…</p>}</div>;

  return <div className="settings-page imports-page">
    <header className="page-head"><h2>导入管理</h2><p className="help">选择要导入的微信会话，并为每个会话设置首次上传的历史日期范围。</p></header>
    <section className="imports-account-card">{accountId ? <WechatIdentity account={{ id: accountId }} profile={profiles[accountId]} /> : <><span className="wechat-account-avatar">微</span><span className="wechat-account-details"><strong>微信账号</strong><small>未检测到微信账号</small></span></>}{accounts.length > 1 && <select aria-label="微信账号" disabled={saving} value={accountId} onChange={event => { if (saveLock.current) return; const id = event.target.value; accountIdRef.current = id; setCatalog([]); setCatalogLoading(Boolean(id)); setAccountId(id); setFocusedSession(''); }}>{accounts.map(account => <option key={account.id} value={account.id}>{profiles[account.id]?.nickname || accountLabel(account)}</option>)}</select>}</section>
    <p className="imports-hint">首次历史日期范围仅用于第一次上传；后续自动备份会继续同步新增消息。选择的微信账号和范围在保存成功后才生效。</p>
    <div className="imports-workspace">
      <div className="imports-choices"><input className="plan-input imports-search" type="search" value={search} placeholder="搜索会话名或 wxid" aria-label="搜索会话" onChange={event => setSearch(event.target.value)}/>
        <div className="plan-pills" role="group" aria-label="按会话类型筛选">
          <button type="button" aria-pressed={kind === 'all'} className={kind === 'all' ? 'on' : ''} onClick={() => setKind('all')}>全部 {matchedCatalog.length}</button>
          <button type="button" aria-pressed={kind === 'group'} className={kind === 'group' ? 'on' : ''} onClick={() => setKind('group')}>群聊 {groupCount}</button>
          <button type="button" aria-pressed={kind === 'direct'} className={kind === 'direct' ? 'on' : ''} onClick={() => setKind('direct')}>单聊 {matchedCatalog.length - groupCount}</button>
        </div>
        <div className="imports-bulk"><button type="button" disabled={!visibleCatalog.length} onClick={() => setVisibleSelection(true)}>全部导入</button><button type="button" disabled={!visibleCatalog.length} onClick={() => setVisibleSelection(false)}>全部忽略</button></div>
        <div className="imports-list">{visibleCatalog.map(session => <div key={session.username} className={`imports-choice${focused?.username === session.username ? ' selected' : ''}`}><input type="checkbox" aria-label={`导入 ${session.display}`} checked={allowed.has(session.username)} onChange={() => toggleUsername(session.username)}/><button type="button" onClick={() => setFocusedSession(session.username)}><strong>{session.display}</strong><small>{session.is_group ? '群聊' : '单聊'} · {session.username}</small></button></div>)}
        {!accountId && <p className="plan-empty">未发现微信账号，请先完成连接。</p>}{accountId && catalogLoading && <p className="plan-empty">正在加载会话…</p>}{accountId && !catalogLoading && !visibleCatalog.length && <p className="plan-empty">没有符合条件的会话。</p>}</div>
      </div>
      <section className="imports-detail">{focused ? <><header><h3>{focused.display}</h3><p>{focused.is_group ? '群聊' : '单聊'} · {focused.username}</p></header>{!allowed.has(focused.username) ? <p className="help">勾选此会话后，可以设置首次上传的历史范围。</p> : alreadySynced ? <p className="help">此会话已有同步进度，首次历史范围不再生效。</p> : <><h3>首次上传范围</h3><label className="imports-all-history"><input type="checkbox" checked={!isCustomRange} onChange={e => setUploadAllHistory(e.target.checked)}/><span><strong>上传全部历史记录</strong><small>首次上传该会话的全部本机历史记录。</small></span></label>{isCustomRange && <><p className="help">选择首次上传的历史日期范围；可只填写开始日期或结束日期。</p><div className="imports-dates"><label>开始日期<input type="date" value={dateText(focusedRange?.since)} onChange={e => setDate(focused.username, 'since', e.target.value)}/></label><span>至</span><label>结束日期<input type="date" value={dateText(focusedRange?.until, true)} onChange={e => setDate(focused.username, 'until', e.target.value)}/></label></div><div className="imports-available"><strong>有记录的日期</strong><button type="button" className="plan-button" disabled={!!dateLoading} onClick={() => void loadDates(focused.username)}>{dateLoading === focused.username ? '正在读取…' : '查看日期'}</button>{dateDetails[focused.username] && (dateDetails[focused.username].length ? <><p className="imports-date-hint">选择日期作为开始日期，结束日期保持不变。</p><div className="imports-date-list" aria-label="选择有记录的开始日期">{dateDetails[focused.username].map(([day]) => <button type="button" key={day} className={dateText(focusedRange?.since) === day ? 'selected' : ''} aria-pressed={dateText(focusedRange?.since) === day} onClick={() => setDate(focused.username, 'since', day)}>{day}</button>)}</div></> : <p>没有可用记录</p>)}</div></>}</>}</> : <p className="help">选择一个会话，查看并设置首次上传的日期范围。</p>}</section>
    </div>
    {notice && <div className="notice">{notice}</div>}{error && <div className="error">{error}</div>}{saveError && <div className="error" role="alert">{saveError}</div>}
    <div className="page-actions"><button className="primary" disabled={saving || !accountId || catalogLoading} onClick={() => void save()}>{saving ? '正在保存…' : '保存所选会话'}</button><span>已选 {allowedCount} 个会话</span></div>
  </div>;
}

export function StrategySettings({ title = '本地自动化', description = '每日总结在本机生成后投递到服务端。上传频率、上传时段由「备份周期」页面单独配置。' }: { title?: string; description?: string } = {}) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [loaded, setLoaded] = useState(false);
  const saveLock = useRef(false);
  const confirmedRevision = useRef<number | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(stored => { const next = mergeSettings(requireSettingsRevision(stored)); confirmedRevision.current = next.user_settings_revision; setSettings(next); setLoaded(true); })
      .catch(e => setError(String(e)));
  }, []);

  const save = async () => {
    if (!loaded || confirmedRevision.current === null || saveLock.current) return;
    saveLock.current = true;
    setSaving(true);
    setSaveError('');
    setError('');
    setNotice('');
    try {
      const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: { ...settings, user_settings_revision: confirmedRevision.current } })));
      confirmedRevision.current = saved.user_settings_revision;
      setSettings(current => mergeSavedSettings(saved, settings, current));
      try {
        const status = await invoke<{ message?: string }>('devify_daily_schedule_sync');
        setNotice(status?.message || '日报计划已同步到服务端。');
      } catch (syncError) {
        setError(`设置已保存，但日报计划同步失败：${String(syncError)}`);
      }
    } catch (e) {
      setSaveError(settingsSaveError(e));
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };

  if (!loaded) return <div className="settings-page">{error ? <div className="error">设置加载失败：{error} 请重新打开页面。</div> : <p>正在加载设置…</p>}</div>;

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
      <p className="help">日报计划保存在服务端，到点后由服务端基于已上传的聊天记录生成；应用关闭也会执行。请先在「账户」登录并选择空间，并确保需要的内容已经上传。</p>
      {settings.automation.last_daily_run && <p className="muted">最近执行：{settings.automation.last_daily_run} · {DAILY_STATUS_LABEL[settings.automation.last_daily_status ?? ''] ?? '未知'}</p>}
      {settings.automation.last_daily_error && <div className="config warn">{settings.automation.last_daily_error}</div>}
      {settings.automation.last_schedule_error && <div className="config warn">计划同步失败：{settings.automation.last_schedule_error}</div>}
    </section>
    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{error}</div>}{saveError && <div className="error" role="alert">{saveError}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? '正在保存…' : '保存并同步日报计划'}</button>
    </div>
  </div>;
}

export function DevifySettings() {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [loaded, setLoaded] = useState(false);
  const confirmed = useRef<AppSettings | null>(null);
  const draftDirty = useRef(false);
  const draftUrl = useRef('');
  const saveQueue = useRef<Promise<void>>(Promise.resolve());
  const queueFailed = useRef(false);
  const pendingSaves = useRef(0);

  const acceptLoadedSettings = (stored: AppSettings) => {
    const next = mergeSettings(requireSettingsRevision(stored));
    if (draftDirty.current || pendingSaves.current || queueFailed.current) {
      if (confirmed.current && next.user_settings_revision > confirmed.current.user_settings_revision && !queueFailed.current) {
        queueFailed.current = true;
        setSaveError('账号、空间或设置已变化，地址草稿未保存。请重新打开页面，确认当前账号和空间后重试。');
      }
      return;
    }
    if (confirmed.current && next.user_settings_revision < confirmed.current.user_settings_revision) return;
    confirmed.current = next;
    draftUrl.current = next.devify.base_url;
    setSettings(next);
    setLoaded(true);
  };
  const [profile, setProfile] = useState<{ display_name?: string; username?: string } | null>(null);
  const [wechatStatus, setWechatStatus] = useState('正在检查…');
  const [wechatAccounts, setWechatAccounts] = useState<Account[]>([]);
  const wechatProfiles = useWechatProfiles(wechatAccounts);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [authStatus, setAuthStatus] = useState('');
  const [authenticated, setAuthenticated] = useState(false);
  const [spaces, setSpaces] = useState<DevifySpace[]>([]);
  const [takeoverReason, setTakeoverReason] = useState('');
  const [spacesStatus, setSpacesStatus] = useState('');

  useEffect(() => {
    let active = true;
    void invoke<ScanResult>('wechat_scan').then(scan => {
      if (active) { setWechatAccounts(scan.accounts); setWechatStatus(scan.accounts.length ? `本机发现 ${scan.accounts.length} 个微信账号` : '未发现微信账号'); }
    }).catch(() => { if (active) setWechatStatus('无法检查微信账号'); });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    let active = true;
    if (authenticated) void invoke<{ display_name?: string; username?: string }>('devify_current_user').then(value => { if (active) setProfile(value); }).catch(() => { if (active) setProfile(null); });
    else setProfile(null);
    return () => { active = false; };
  }, [authenticated]);

  useEffect(() => {
    void invoke<AppSettings>('app_settings_get')
      .then(acceptLoadedSettings)
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
          acceptLoadedSettings(await invoke<AppSettings>('app_settings_get'));
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
        acceptLoadedSettings(await invoke<AppSettings>('app_settings_get'));
        setAuthStatus('服务端已登录。');
        setSpacesStatus('正在读取可用空间…');
        const available = await invoke<DevifySpace[]>('devify_spaces');
        if (!active) return;
        setSpaces(available);
        setSpacesStatus(available.length ? `已读取 ${available.length} 个空间。` : '当前账号没有可用空间。');
        if (!queueFailed.current) setError('');
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

  const loginDevify = async () => {
    if (draftDirty.current && !pendingSaves.current && !queueFailed.current) {
      persist({ ...settings, devify: { ...settings.devify, base_url: draftUrl.current } });
    }
    while (pendingSaves.current) await saveQueue.current;
    if (queueFailed.current || draftDirty.current) {
      setSaveError(current => current || '服务地址尚未确认保存，不能继续登录。请确认地址、账号和空间后重试。');
      return;
    }
    setError('');
    setAuthStatus('正在打开服务端登录…');
    try {
      const url = await invoke<string>('devify_auth_start', { deviceLabel: settings.devify.device_label });
      await invoke('devify_auth_open', { url });
    } catch (e) {
      setAuthStatus('');
      setError(String(e));
    }
  };

  const logoutDevify = async () => {
    if (!queueFailed.current) setError('');
    try {
      await invoke('devify_logout');
      setAuthenticated(false);
      if (draftDirty.current || pendingSaves.current) { queueFailed.current = true; setSaveError('登录状态已变化，地址草稿未保存。请重新打开页面并确认账号和空间。'); }
      setSpaces([]);
      setSpacesStatus('');
      setAuthStatus('已退出登录。');
      window.dispatchEvent(new CustomEvent('aimychats:auth-changed', { detail: { authenticated: false } }));
    } catch (e) { setError(String(e)); }
  };

  const loadSpaces = async () => {
    setSpacesStatus('正在读取可用空间…');
    try {
      const available = await invoke<DevifySpace[]>('devify_spaces');
      setSpaces(available);
      setSpacesStatus(available.length ? `已读取 ${available.length} 个空间。` : '当前账号没有可用空间。');
    } catch (e) { setSpaces([]); setSpacesStatus('读取空间失败。'); setError(String(e)); }
  };

  const selectedSpace = spaces.find(space => space.id === settings.devify.active_space_id);
  const activeSpace = selectedSpace;
  const platformName = ({ macos: 'macOS', windows: 'Windows', linux: 'Linux' } as Record<string, string>)[settings.devify.platform] || settings.devify.platform || '自动识别';
  const needsTakeover = Boolean(selectedSpace?.is_global_admin && !selectedSpace.admin_takeover_active);
  const takeoverSpace = async () => {
    if (!selectedSpace || !takeoverReason.trim()) return;
    try {
      await invoke('devify_space_takeover', { spaceId: selectedSpace.id, reason: takeoverReason.trim() });
      setTakeoverReason('');
      await loadSpaces();
    } catch (e) { setError(String(e)); }
  };

  /** Queue blur saves against our own last confirmed revision, never a fresh external context. */
  const persist = (next: AppSettings) => {
    if (!confirmed.current || queueFailed.current) return;
    pendingSaves.current += 1;
    saveQueue.current = saveQueue.current.then(async () => {
      if (queueFailed.current || !confirmed.current) return;
      const base = confirmed.current;
      const submitted = { ...base, devify: { ...base.devify, base_url: displayBaseUrl(next.devify.base_url) || next.devify.base_url } };
      try {
        const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: submitted })));
        confirmed.current = saved;
        draftDirty.current = draftUrl.current !== next.devify.base_url;
        if (!draftDirty.current && pendingSaves.current === 1) draftUrl.current = saved.devify.base_url;
        setSettings(current => mergeSavedSettings(saved, next, current));
        if (!queueFailed.current) setSaveError('');
      } catch (reason) {
        queueFailed.current = true;
        setSaveError(`${settingsSaveError(reason)} 地址草稿已保留；请重新打开页面确认后保存。`);
      }
    }).finally(() => { pendingSaves.current -= 1; });
  };

  if (!loaded) return <div className="settings-page">{error ? <div className="error">设置加载失败：{error} 请重新打开页面。</div> : <p>正在加载设置…</p>}</div>;

  return <div className="settings-page account-settings">
    <header className="page-head">
      <h2>账户</h2>
      <p className="help">管理 AIMyChats 账号、服务连接和微信数据来源。</p>
    </header>

    <section className="account-profile-card account-identity"><h3>AIMyChats 账户</h3><div className="account-profile-content">
      <span className="account-profile-avatar">{(profile?.display_name || profile?.username || '账').slice(0, 1)}</span>
      <span className="account-profile-info"><strong>{authenticated ? (profile?.display_name || profile?.username || '账号信息暂不可用') : '未登录'}</strong><small>{profile?.username ? `@${profile.username}` : 'AIMyChats 账号'}</small></span>
      <span className={`account-login-status${authenticated ? ' is-active' : ''}`}><i/>{authenticated ? '已登录' : '未登录'}</span>
      {authenticated ? <button type="button" className="plan-button account-auth-action" onClick={() => void logoutDevify()}>退出登录</button> : <button type="button" className="plan-button primary account-auth-action" onClick={() => void loginDevify()}>登录</button>}
    </div>{authStatus && <p className="account-auth-notice" role="status">{authStatus}</p>}</section>
    <section className="account-service-card"><h3>服务与空间</h3><div className="plan-items">
      <div className="plan-row">
        <div className="plan-row-label">
          <span>服务地址</span>
          <small>AIMyChats 服务的访问地址</small>
        </div>
        <div className="plan-row-control">
          <input
            className="plan-input"
            type="url"
            aria-label="服务端地址"
            placeholder="https://devify.example.com"
            value={settings.devify.base_url}
            onChange={event => { const baseUrl = event.target.value; draftDirty.current = true; draftUrl.current = baseUrl; setSettings(current => ({ ...current, devify: { ...current.devify, base_url: baseUrl } })); }}
            onBlur={() => persist({ ...settings, devify: { ...settings.devify, base_url: draftUrl.current } })}
          />
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>当前空间</span>
          <small>{activeSpace ? '聊天记录、事件与报告所属的空间' : (spacesStatus || '登录后自动选择个人空间')}</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{activeSpace ? `${activeSpace.name} · ${activeSpace.role || activeSpace.kind}` : (authenticated ? (spacesStatus || '正在读取空间…') : '未登录')}</span>
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>当前设备</span>
          <small>正在使用的设备</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{[settings.devify.device_label || '本机', platformName].filter(Boolean).join(' · ')}</span>
        </div>
      </div>

    </div></section>

    <section className="account-profile-card account-wechat"><h3>微信数据来源</h3>{wechatAccounts.length ? wechatAccounts.map(account => <div key={account.id} className="wechat-source-account"><WechatIdentity account={account} profile={wechatProfiles[account.id]} /></div>) : <div><span className="wechat-account-avatar">微</span><span><strong>{wechatStatus}</strong><small>微信账号与 AIMyChats 账号相互独立，用于读取本机聊天记录。</small></span></div>}</section>
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
    {saveError && <div className="error" role="alert">{saveError}</div>}
  </div>;
}
