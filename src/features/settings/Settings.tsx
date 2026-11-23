import { t, tf, ctf, dateLocale, sortLocale } from '../../shared/i18n/locale';
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { onOpenUrl } from '@tauri-apps/plugin-deep-link';
import { registerDevifyDevice } from '../../shared/api/devifyDevice';
import { sameServiceUrl, storedBaseUrl } from '../../shared/api/serviceUrl.ts';
import type { UploadOwner } from '../tasks/taskCloud';
import { defaultSettings, mergeSettings, mergeSavedSettings, requireSettingsRevision, settingsSaveError, type AppSettings } from './settingsModel';
import { AccountPicker } from '../connectors/AccountPicker';
import { ConnectorIcon } from '../connectors/Connectors';
import { WizardSessionPicker } from '../connectors/wizard/WizardSessionPicker';
import { WizardImportScope } from '../connectors/wizard/WizardImportScope';
import { Icons } from '../../shared/ui/icons';
import { ImportHistoryRange, presetHistoryRange, type HistoryRange, type HistoryChoice, type HistoryScope } from '../connectors/wizard/ImportHistoryRange';

type SessionItem = {
  username: string;
  display: string;
  last_time: number | null;
  is_group: boolean;
  member_count: number | null;
  preview: string | null;
  unread: number | null;
};

type Account = { id: string; modified_at?: number | null; source?: string; display?: string | null };
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

function WechatIdentity({ account, profile, source }: { account: Account; profile?: WechatProfile; source?: string }) {
  const whatsapp = source === 'whatsapp';
  return <><span className={`wechat-account-avatar${whatsapp ? ' whatsapp' : ''}`}>{profile?.avatar ? <img src={profile.avatar} alt="" /> : <ConnectorIcon name={whatsapp ? 'WhatsApp' : '微信'} />}</span><span className="wechat-account-details"><strong>{profile?.nickname || (whatsapp ? 'WhatsApp' : t("微信账号"))}</strong><small>{whatsapp ? `WhatsApp${profile?.alias ? ` · ${profile.alias}` : ''}` : profile?.alias ? tf("微信号：{0}", profile.alias) : t("微信号暂不可用")}</small><small>{t("本机标识：")}{account.id}</small></span></>;
}
type DevifySpace = { id: string; name: string; kind: string; status: string; role?: string };

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

function accountLabel(account: Account): string {
  return account.id.length > 38 ? `${account.id.slice(0, 35)}…` : account.id;
}

type SyncIdentity = { user_id: number; device_id: string };

function importCursors(settings: AppSettings, accountId: string, identity: SyncIdentity | null) {
  const matches = (owner?: UploadOwner | null) => Boolean(owner && identity
    && owner.account_id === accountId && owner.space_id === settings.devify.active_space_id
    && owner.user_id === identity.user_id && owner.device_id === identity.device_id
    && sameServiceUrl(owner.base_url, settings.devify.base_url));
  return matches(settings.sync.owners?.[accountId])
    ? settings.sync.cursors?.[accountId]
    : settings.sync.saved_progress?.find(item => matches(item.owner))?.cursors;
}

export function WhitelistSettings({ onSaved, wizard = false, wizardSource = 'wechat', onBack }: { onSaved?: (count: number) => void; wizard?: boolean; wizardSource?: 'wechat' | 'whatsapp'; onBack?: () => void } = {}) {
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [loaded, setLoaded] = useState(false);
  const saveLock = useRef(false);
  const confirmedRevision = useRef<number | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const profiles = useWechatProfiles(accounts);
  const [accountId, setAccountId] = useState('');
  const [syncIdentity, setSyncIdentity] = useState<SyncIdentity | null>(null);
  const identityRequest = useRef<Promise<SyncIdentity | null> | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const accountIdRef = useRef('');
  const [catalog, setCatalog] = useState<SessionItem[]>([]);
  const [catalogLoading, setCatalogLoading] = useState(false);
  const [kind, setKind] = useState<'all' | 'group' | 'direct'>('all');
  const [search, setSearch] = useState('');
  const currentCursors = importCursors(settings, accountId, syncIdentity);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [selectionIssue, setSelectionIssue] = useState<{ attempted: number } | null>(null);
  const [importsExpanded, setImportsExpanded] = useState(true);
  const [dateDetails, setDateDetails] = useState<Record<string, Record<string, Array<[string, number]>>>>({});
  const [dateLoading, setDateLoading] = useState('');
  const [focusedSession, setFocusedSession] = useState('');
  const [historyChoices, setHistoryChoices] = useState<Record<string, Record<string, HistoryChoice>>>({});
  const [historyScope, setHistoryScope] = useState<HistoryScope>('selected');

  // Same-day activity reads better with a clock than with a repeated date.
  const activityText = (seconds: number | null | undefined) => {
    if (!seconds) return t("无记录");
    const date = new Date(seconds * 1000);
    return date.toDateString() === new Date().toDateString()
      ? tf("今天 {0}", date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }))
      : date.toLocaleDateString(dateLocale(), { month: 'numeric', day: 'numeric' });
  };
  const loadDates = async (username: string) => {
    setDateLoading(username); setError('');
    try { const dates = await invoke<Array<[string, number]>>('wechat_catalog_dates', { accountId, username }); setDateDetails(current => ({ ...current, [accountId]: { ...current[accountId], [username]: dates } })); }
    catch (reason) { if (accountIdRef.current === accountId) setError(`读取有记录的日期失败：${String(reason)}`); }
    finally { if (accountIdRef.current === accountId) setDateLoading(''); }
  };

  const allowed = useMemo(
    () => new Set(settings.chat.allowed_usernames[accountId] ?? []),
    [settings.chat.allowed_usernames, accountId],
  );

  // Activity order: newest message first. Sessions without a timestamp sink to the
  // bottom instead of interleaving with real activity.
  const matchedCatalog = useMemo(() => {
    const keyword = search.trim().toLowerCase();
    return catalog.filter(session => !keyword || session.display.toLowerCase().includes(keyword) || session.username.toLowerCase().includes(keyword))
      .sort((a, b) => (b.last_time ?? 0) - (a.last_time ?? 0) || a.display.localeCompare(b.display, sortLocale()));
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
    let active = true;
    void (async () => {
      try {
        const [stored, scan] = await Promise.all([
          invoke<AppSettings>('app_settings_get'),
          invoke<Account[]>('chat_accounts'),
        ]);
        if (!active) return;
        const next = mergeSettings(requireSettingsRevision(stored));
        identityRequest.current = Object.keys(next.sync.owners ?? {}).length || next.sync.saved_progress?.length
          ? invoke<SyncIdentity>('devify_upload_identity').catch(() => null)
          : null;
        const list = wizard ? (scan ?? []).filter(account => wizardSource === 'whatsapp' ? account.source === 'whatsapp' : account.source !== 'whatsapp') : scan ?? [];
        confirmedRevision.current = next.user_settings_revision;
        setSettings(next);
        setLoaded(true);
        setAccounts(list);
        const id = currentAccount(list, next.active_account_id)?.id ?? '';
        accountIdRef.current = id;
        setCatalogLoading(Boolean(id));
        setHistoryLoading(Boolean(id));
        setAccountId(id);
      } catch (e) {
        if (active) setError(String(e));
      }
    })();
    return () => { active = false; };
  }, []);

  useEffect(() => {
    if (!accountId) return;
    let active = true;
    setCatalog([]);
    setCatalogLoading(true);
    setError('');
    void invoke<SessionItem[]>('wechat_session_catalog', { accountId })
      .then(async items => {
        if (!active || accountIdRef.current !== accountId) return;
        setCatalog(items);
        setCatalogLoading(false);
        // Only history settings depend on verifying ownership of stored backup progress.
        const hasProgress = Boolean(settings.sync.owners?.[accountId]
          || settings.sync.saved_progress?.some(item => item.owner?.account_id === accountId));
        const identity = hasProgress ? await identityRequest.current : null;
        if (!active || accountIdRef.current !== accountId) return;
        setSyncIdentity(identity);
        // Offer the complete current catalog only for an unconfigured account. Nothing
        // is persisted until Next; existing saved choices and the backend cap stay intact.
        setSettings(current => {
          const selected = current.chat.allowed_usernames[accountId]
            ?? (wizard && wizardSource === 'wechat' && items.length > 0 && items.length <= 1000 ? items.map(item => item.username) : []);
          const ranges = { ...current.chat.initial_ranges[accountId] };
          const cursors = importCursors(current, accountId, identity);
          for (const username of selected) if (!ranges[username] && !cursors?.[username]) ranges[username] = presetHistoryRange('7');
          return { ...current, chat: { ...current.chat,
            allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: selected },
            initial_ranges: { ...current.chat.initial_ranges, [accountId]: ranges },
          } };
        });
      })
      .catch(e => {
        if (!active || accountIdRef.current !== accountId) return;
        setCatalog([]);
        setError(tf("无法读取该账号的会话候选：{0}", String(e)));
      })
      .finally(() => {
        if (!active || accountIdRef.current !== accountId) return;
        setCatalogLoading(false);
        setHistoryLoading(false);
      });
    return () => { active = false; };
  }, [accountId]);

  const visibleUsernames = useMemo(() => visibleCatalog.map(session => session.username), [visibleCatalog]);
  // Bulk actions only touch what the current search/type filter shows, so "全选"
  // after filtering never silently changes sessions the user cannot see.
  const setVisibleSelection = (mode: 'select' | 'clear' | 'invert', usernames = visibleUsernames) => {
    setNotice('');
    const next = new Set(settings.chat.allowed_usernames[accountId] ?? []);
    if (mode === 'select') {
      usernames.forEach(username => next.add(username));
    } else if (mode === 'clear') {
      usernames.forEach(username => next.delete(username));
    } else {
      usernames.forEach(username => (next.has(username) ? next.delete(username) : next.add(username)));
    }
    if (next.size > 1000) {
      setSelectionIssue({ attempted: next.size });
      setSaveError('');
      return;
    }
    setSelectionIssue(null);
    setSaveError('');
    setSettings(current => {
      const ranges = { ...current.chat.initial_ranges[accountId] };
      for (const username of next) if (!ranges[username] && !currentCursors?.[username]) ranges[username] = presetHistoryRange('7');
      return { ...current, chat: { ...current.chat, allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] }, initial_ranges: { ...current.chat.initial_ranges, [accountId]: ranges } } };
    });
  };

  const toggleUsername = (username: string) => {
    setNotice('');
    if (!allowed.has(username) && allowedCount >= 1000) {
      setSelectionIssue({ attempted: allowedCount + 1 });
      setSaveError('');
      return;
    }
    setSelectionIssue(null);
    setSaveError('');
    if (!allowed.has(username)) setFocusedSession(username);
    setSettings(current => {
      const next = new Set(current.chat.allowed_usernames[accountId] ?? []);
      if (next.has(username)) next.delete(username);
      else next.add(username);
      const ranges = { ...current.chat.initial_ranges[accountId] };
      if (next.has(username) && !ranges[username] && !currentCursors?.[username]) ranges[username] = presetHistoryRange('7');
      return { ...current, chat: { ...current.chat, allowed_usernames: { ...current.chat.allowed_usernames, [accountId]: [...next] }, initial_ranges: { ...current.chat.initial_ranges, [accountId]: ranges } } };
    });
  };

  const setScopeSelection = (group: boolean, select: boolean) => {
    setHistoryScope('selected');
    setVisibleSelection(select ? 'select' : 'clear', catalog.filter(item => item.is_group === group).map(item => item.username));
  };

  const save = async () => {
    if (!loaded || !accountId || catalogLoading || historyLoading || confirmedRevision.current === null || saveLock.current) return;
    saveLock.current = true;
    setSaving(true);
    setSelectionIssue(null);
    setSaveError('');
    setError('');
    setNotice('');
    try {
      // Saving submits every account draft, so validate all selected drafts too.
      const missingRange = Object.entries(historyChoices).some(([id, choices]) =>
        (settings.chat.allowed_usernames[id] ?? []).some(username => {
          const range = settings.chat.initial_ranges[id]?.[username];
          return choices[username] === 'custom' && range?.since == null && range?.until == null;
        }));
      if (missingRange) throw new Error('请选择自定义范围的开始或结束日期，或选择「全部历史」。');
      const invalidRange = Object.entries(settings.chat.allowed_usernames).some(([id, usernames]) => usernames.some(username => {
        const range = settings.chat.initial_ranges[id]?.[username];
        return range?.since != null && range.until != null && range.since >= range.until;
      }));
      if (invalidRange) throw new Error('历史记录结束日期不能早于开始日期。');
      const keep = catalog.length
        ? (settings.chat.allowed_usernames[accountId] ?? []).filter(username => catalogUsernames.has(username))
        : settings.chat.allowed_usernames[accountId];
      const invalidId = (keep ?? []).find(username => !username || new TextEncoder().encode(username).length > 512);
      if (invalidId !== undefined) throw new Error(tf("会话 ID 无效或超过 512 字节：{0}", invalidId.slice(0, 60) || '空 ID'));
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
      setNotice(`已保存 ${(keep ?? []).length} 个会话，重新打开聊天记录页面后生效。`);
      onSaved?.((keep ?? []).length);
    } catch (e) {
      setSaveError(settingsSaveError(e));
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };

  const focused = catalog.find(session => session.username === focusedSession) ?? catalog.find(session => allowed.has(session.username));
  const applyHistoryRange = (usernames: string[], range: HistoryRange, choice: HistoryChoice) => {
    if (saveLock.current || accountIdRef.current !== accountId) return;
    const targets = usernames.filter(username => allowed.has(username) && catalogUsernames.has(username) && !currentCursors?.[username]);
    if (!targets.length) return;
    setNotice('');
    setSaveError('');
    setHistoryChoices(current => ({ ...current, [accountId]: { ...current[accountId], ...Object.fromEntries(targets.map(username => [username, choice])) } }));
    setSettings(current => ({ ...current, chat: { ...current.chat, initial_ranges: { ...current.chat.initial_ranges,
      [accountId]: { ...current.chat.initial_ranges[accountId], ...Object.fromEntries(targets.map(username => [username, { ...range }])) },
    } } }));
  };

  const changeAccount = (id: string) => {
    if (saveLock.current) return;
    accountIdRef.current = id;
    setCatalog([]);
    setCatalogLoading(Boolean(id));
    setHistoryLoading(Boolean(id));
    setAccountId(id);
    setFocusedSession('');
    setHistoryScope('selected');
    setDateLoading('');
    setSelectionIssue(null);
    setSaveError('');
    setNotice('');
    setSearch('');
    setKind('all');
    // Keep all account drafts in the same form; save once using its confirmed revision.
    setSettings(current => ({ ...current, active_account_id: id }));
  };

  const wizardWechat = wizard && wizardSource === 'wechat';
  const historyEditor = <ImportHistoryRange selected={catalog.filter(item => allowed.has(item.username))} focused={focused}
    synced={new Set(Object.keys(currentCursors ?? {}).filter(username => !!currentCursors?.[username]))}
    scope={historyScope} ranges={settings.chat.initial_ranges[accountId] ?? {}} choices={historyChoices[accountId] ?? {}} disabled={saving}
    onScope={setHistoryScope} onFocus={id => { setFocusedSession(id); setHistoryScope('single'); }} onChange={applyHistoryRange}
    dateDetails={dateDetails[accountId] ?? {}} dateLoading={dateLoading} onLoadDates={id => void loadDates(id)} />;

  const source = accounts.find(account => account.id === accountId)?.source === 'whatsapp' ? 'whatsapp' : 'wechat';
  const sourceName = source === 'whatsapp' ? 'WhatsApp' : '微信';
  const sourceAccounts = accounts.filter(account => (account.source === 'whatsapp' ? 'whatsapp' : 'wechat') === source);
  const editor = <>
    {(!wizardWechat || accounts.length > 1 || !accountId) && <section className="imports-account-card">{accountId ? <WechatIdentity account={{ id: accountId }} profile={profiles[accountId]} source={source} /> : <><span className="wechat-account-avatar"><ConnectorIcon name={sourceName} /></span><span className="wechat-account-details"><strong>{t("聊天账号")}</strong><small>{t("未检测到可用的聊天账号")}</small></span></>}{sourceAccounts.length > 1 && <AccountPicker accounts={sourceAccounts} value={accountId} disabled={saving} nameFor={account => profiles[account.id]?.nickname || accountLabel(account)} onChange={changeAccount} />}</section>}
    {source === 'whatsapp' && <p className="imports-hint">{t("选择的账号和范围同步成功后，将按备份周期自动执行。")}</p>}
    <WizardImportScope catalog={catalog} selected={allowed} loading={catalogLoading} disabled={saving || historyLoading} accountId={accountId} onScope={setScopeSelection} selectionIssue={selectionIssue} onClearIssue={() => setSelectionIssue(null)} sourceName={sourceName}>
      <WizardSessionPicker items={visibleCatalog} total={matchedCatalog.length} groupCount={groupCount}
        selected={allowed} selectedCount={allowedCount} kind={kind} onKind={setKind} search={search} onSearch={setSearch}
        loading={catalogLoading} disabled={saving || historyLoading} accountId={accountId} onToggle={toggleUsername} onBulk={setVisibleSelection}
        focused={focused?.username ?? ''} onFocus={id => { setFocusedSession(id); setHistoryScope('single'); }} activity={activityText} sourceName={sourceName} />
    </WizardImportScope>
    {!catalogLoading && accountId && (historyLoading
      ? <p className="help" role="status">{t("正在核对备份进度…")}</p>
      : historyEditor)}
    {error && <div className="error">{error}</div>}{saveError && <div className="error" role="alert">{saveError}</div>}{allowedCount > 1000 && <div className="error" role="alert">{t("已超出")}{allowedCount - 1000} {t("个会话，请取消选择后保存。")}</div>}
    <div className="page-actions">{notice && <span className="imports-save-status" role="status">{Icons.check}<span>{notice}</span></span>}{wizard && onBack && <button type="button" className="wxv-btn" disabled={saving} onClick={onBack}>{t("上一步")}</button>}<button className="primary" aria-busy={saving} disabled={saving || !accountId || catalogLoading || historyLoading || allowedCount > 1000 || (wizard && allowedCount === 0)} onClick={() => void save()}>{saving ? t("正在保存…") : wizardWechat ? t("下一步") : wizard ? t("保存并继续") : t("保存所选会话")}</button></div>
  </>;
  return <div className={`settings-page imports-page imports-v7${wizard ? ' wizard-imports' : ' imports-management'}`}>
    <header className="page-head"><h2>{wizard ? t("选择要备份的会话") : t("导入管理")}</h2><p className="help">{wizard ? t("只导入你确认的会话。会话较多时，可以通过「自定义」筛选和批量选择。") : t("每个平台单独选择要备份的私聊和群聊。连接向导里也可以设置，修改后保存即可生效。")}</p></header>
    {!loaded ? (error
      ? <div className="error" role="alert">{t("设置加载失败：")}{error} {t("请重新打开页面。")}</div>
      : <p role="status">{t("正在加载设置…")}</p>)
      : wizard || !accounts.length ? editor : <div className="imports-platforms">{(['wechat', 'whatsapp'] as const).filter(platform => accounts.some(account => (account.source === 'whatsapp' ? 'whatsapp' : 'wechat') === platform)).map(platform => {
      const list = accounts.filter(account => (account.source === 'whatsapp' ? 'whatsapp' : 'wechat') === platform);
      const name = platform === 'whatsapp' ? 'WhatsApp' : t("微信");
      const open = source === platform && importsExpanded;
      const count = list.reduce((total, account) => total + (account.id === accountId ? allowedCount : (settings.chat.allowed_usernames[account.id] ?? []).length), 0);
      return <section key={platform} className="imports-platform">
        <button type="button" className="imports-platform-head" aria-expanded={open} disabled={saving} onClick={() => {
          if (source === platform) setImportsExpanded(!importsExpanded);
          else { changeAccount(currentAccount(list, settings.active_account_id)!.id); setImportsExpanded(true); }
        }}><ConnectorIcon name={platform === 'whatsapp' ? 'WhatsApp' : '微信'} /><span><strong>{name}</strong><small>
          {count === 1 ? t("已选 1 个会话") : tf("已选 {0} 个会话", count)} · {list.length === 1 ? t("1 个账号") : tf("{0} 个账号", list.length)}
        </small></span><svg className={open ? 'open' : ''} width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><path d="m9 5 7 7-7 7" /></svg></button>
        {open && <div className="imports-platform-content">{editor}</div>}
      </section>;
    })}</div>}
  </div>;
}

export function StrategySettings({ title = t("本地自动化"), description = t("每日总结在本机生成后投递到服务端。上传频率、上传时段由「备份周期」页面单独配置。") }: { title?: string; description?: string } = {}) {
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
        setNotice(status?.message || t("日报计划已同步到服务端。"));
      } catch (syncError) {
        setError(tf("设置已保存，但日报计划同步失败：{0}", String(syncError)));
      }
    } catch (e) {
      setSaveError(settingsSaveError(e));
    } finally {
      saveLock.current = false;
      setSaving(false);
    }
  };

  if (!loaded) return <div className="settings-page">{error ? <div className="error">{t("设置加载失败：")}{t(error)} {t("请重新打开页面。")}</div> : <p>{t("正在加载设置…")}</p>}</div>;

  return <div className="settings-page">
    <header className="page-head">
      <h2>{title}</h2>
      <p className="help">{t(description)}</p>
    </header>
    <section className="panel">
      <label className="confirm"><input type="checkbox" checked={settings.automation.enabled} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, enabled: event.target.checked } }))}/><span>{t("启用自动化调度")}</span></label>
      <div className="grid">
        <label>{t("日报时间")}
          <input type="time" value={settings.automation.daily_time} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, daily_time: event.target.value } }))}/>
        </label>
        <label>{t("每日日报")}
          <select value={settings.automation.daily_summary_enabled ? 'on' : 'off'} onChange={event => setSettings(current => ({ ...current, automation: { ...current.automation, daily_summary_enabled: event.target.value === 'on' } }))}>
            <option value="on">{t("生成")}</option>
            <option value="off">{t("暂不生成")}</option>
          </select>
        </label>
      </div>
      <p className="help">{t("日报计划保存在服务端，到点后由服务端基于已上传的聊天记录生成；应用关闭也会执行。请先在「账户」登录并选择空间，并确保需要的内容已经上传。")}</p>
      {settings.automation.last_daily_run && <p className="muted">{t("最近执行：")}{settings.automation.last_daily_run} · {DAILY_STATUS_LABEL[settings.automation.last_daily_status ?? ''] ?? t("未知")}</p>}
      {settings.automation.last_daily_error && <div className="config warn">{settings.automation.last_daily_error}</div>}
      {settings.automation.last_schedule_error && <div className="config warn">{t("计划同步失败：")}{settings.automation.last_schedule_error}</div>}
    </section>
    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error">{t(error)}</div>}{saveError && <div className="error" role="alert">{saveError}</div>}
    <div className="page-actions">
      <button className="primary" disabled={saving} onClick={() => void save()}>{saving ? t("正在保存…") : t("保存并同步日报计划")}</button>
    </div>
  </div>;
}

export function DevifySettings({ initialUser = '', initialAuthenticated = false, connection = 'unavailable' }: { initialUser?: string; initialAuthenticated?: boolean; connection?: 'connected' | 'unavailable' }) {
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
        setSaveError(t("账号、空间或设置已变化，地址草稿未保存。请重新打开页面，确认当前账号和空间后重试。"));
      }
      return;
    }
    if (confirmed.current && next.user_settings_revision < confirmed.current.user_settings_revision) return;
    confirmed.current = next;
    draftUrl.current = next.devify.base_url;
    setSettings(next);
    setLoaded(true);
  };
  const [profile, setProfile] = useState<{ display_name?: string; username?: string } | null>(initialUser ? { display_name: initialUser } : null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [saveError, setSaveError] = useState('');
  const [authStatus, setAuthStatus] = useState('');
  const [authenticated, setAuthenticated] = useState(initialAuthenticated);
  const [spaces, setSpaces] = useState<DevifySpace[]>([]);
  const [spacesStatus, setSpacesStatus] = useState('');

  useEffect(() => {
    let active = true;
    if (authenticated) void invoke<{ display_name?: string; username?: string }>('devify_current_user').then(value => { if (active) setProfile(value); }).catch(() => {});
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
    if (initialAuthenticated) void (async () => {
      await registerDevifyDevice();
      if (!active) return;
      await invoke<string | null>('devify_space_ensure').catch(() => null);
      if (!active) return;
      acceptLoadedSettings(await invoke<AppSettings>('app_settings_get'));
      if (active) await loadSpaces();
    })().catch(() => {});
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
        await invoke<string | null>('devify_space_ensure').catch(() => null);
        acceptLoadedSettings(await invoke<AppSettings>('app_settings_get'));
        setAuthStatus(t("服务端已登录。"));
        setSpacesStatus(t("正在读取可用空间…"));
        const available = await invoke<DevifySpace[]>('devify_spaces');
        if (!active) return;
        setSpaces(available);
        setSpacesStatus(available.length ? tf("已读取 {0} 个空间。", available.length) : t("当前账号没有可用空间。"));
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
      setSaveError(current => current || t("服务地址尚未确认保存，不能继续登录。请确认地址、账号和空间后重试。"));
      return;
    }
    setError('');
    setAuthStatus(t("正在打开服务端登录…"));
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
      if (draftDirty.current || pendingSaves.current) { queueFailed.current = true; setSaveError(t("登录状态已变化，地址草稿未保存。请重新打开页面并确认账号和空间。")); }
      setSpaces([]);
      setSpacesStatus('');
      setAuthStatus(t("已退出登录。"));
      window.dispatchEvent(new CustomEvent('aimychats:auth-changed', { detail: { authenticated: false } }));
    } catch (e) { setError(String(e)); }
  };

  const loadSpaces = async () => {
    setSpacesStatus(t("正在读取可用空间…"));
    try {
      const available = await invoke<DevifySpace[]>('devify_spaces');
      setSpaces(available);
      setSpacesStatus(available.length ? tf("已读取 {0} 个空间。", available.length) : t("当前账号没有可用空间。"));
    } catch (e) { setSpaces([]); setSpacesStatus(t("读取空间失败。")); setError(String(e)); }
  };

  const selectedSpace = spaces.find(space => space.id === settings.devify.active_space_id);
  const activeSpace = selectedSpace;
  const platformName = ({ macos: 'macOS', windows: 'Windows', linux: 'Linux' } as Record<string, string>)[settings.devify.platform] || settings.devify.platform || t("自动识别");

  /** Queue blur saves against our own last confirmed revision, never a fresh external context. */
  const persist = (next: AppSettings) => {
    if (!confirmed.current || queueFailed.current) return;
    pendingSaves.current += 1;
    saveQueue.current = saveQueue.current.then(async () => {
      if (queueFailed.current || !confirmed.current) return;
      const base = confirmed.current;
      const submitted = { ...base, devify: { ...base.devify, base_url: storedBaseUrl(next.devify.base_url) } };
      try {
        const saved = mergeSettings(requireSettingsRevision(await invoke<AppSettings>('app_settings_save', { value: submitted })));
        confirmed.current = saved;
        draftDirty.current = draftUrl.current !== next.devify.base_url;
        if (!draftDirty.current && pendingSaves.current === 1) draftUrl.current = saved.devify.base_url;
        setSettings(current => mergeSavedSettings(saved, next, current));
        if (!queueFailed.current) setSaveError('');
      } catch (reason) {
        queueFailed.current = true;
        setSaveError(tf("{0} 地址草稿已保留；请重新打开页面确认后保存。", settingsSaveError(reason)));
      }
    }).finally(() => { pendingSaves.current -= 1; });
  };

  if (!loaded) return <div className="settings-page settings-refreshed account-settings">{error ? <div className="error">{t("设置加载失败：")}{t(error)} {t("请重新打开页面。")}</div> : <p>{t("正在加载设置…")}</p>}</div>;

  return <div className="settings-page settings-refreshed account-settings">
    <header className="page-head">
      <h2>{t("账户")}</h2>
      <p className="help">{t("管理 AIMyChats 账号、服务连接和空间。")}</p>
    </header>

    <section className="account-profile-card account-identity"><h3>{t("AIMyChats 账户")}</h3><div className="account-profile-content">
      <span className="account-profile-avatar">{(profile?.display_name || profile?.username || t("账")).slice(0, 1)}</span>
      <span className="account-profile-info"><strong>{authenticated ? (profile?.display_name || profile?.username || t("账号信息暂不可用")) : t("未登录")}</strong><small>{profile?.username ? `@${profile.username}` : t("AIMyChats 账号")}</small></span>
      <span className={`account-login-status${connection === 'connected' ? ' is-active' : ''}`}><i/>{!authenticated ? t("未登录") : connection === 'connected' ? t("已登录 · 已连接") : t("服务连接异常")}</span>
      {authenticated ? <button type="button" className="plan-button account-auth-action" onClick={() => void logoutDevify()}>{t("退出登录")}</button> : <button type="button" className="plan-button primary account-auth-action" onClick={() => void loginDevify()}>{t("登录")}</button>}
    </div>{authStatus && <p className="account-auth-notice" role="status">{authStatus}</p>}</section>
    <section className="account-service-card"><h3>{t("服务与空间")}</h3><div className="plan-items">
      <div className="plan-row">
        <div className="plan-row-label">
          <span>{t("服务地址")}</span>
          <small>{t("AIMyChats 服务的访问地址")}</small>
        </div>
        <div className="plan-row-control">
          <input
            className="plan-input"
            type="url"
            aria-label={t("服务端地址")}
            placeholder="https://devify.example.com"
            value={settings.devify.base_url}
            onChange={event => { const baseUrl = event.target.value; draftDirty.current = true; draftUrl.current = baseUrl; setSettings(current => ({ ...current, devify: { ...current.devify, base_url: baseUrl } })); }}
            onBlur={() => persist({ ...settings, devify: { ...settings.devify, base_url: draftUrl.current } })}
          />
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>{t("当前空间")}</span>
          <small>{activeSpace ? t("聊天记录、待办与报告所属的空间") : (spacesStatus || t("登录后自动选择个人空间"))}</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{activeSpace ? `${activeSpace.name} · ${activeSpace.role || activeSpace.kind}` : (authenticated ? (spacesStatus || t("正在读取空间…")) : t("未登录"))}</span>
        </div>
      </div>

      <div className="plan-row">
        <div className="plan-row-label">
          <span>{t("当前设备")}</span>
          <small>{t("正在使用的设备")}</small>
        </div>
        <div className="plan-row-control">
          <span className="plan-kv">{[settings.devify.device_label || t("本机"), platformName].filter(Boolean).join(' · ')}</span>
        </div>
      </div>

    </div></section>

    {notice && <div className="notice">{notice}</div>}
    {error && <div className="error" role="alert">{t(error)}</div>}
    {saveError && <div className="error" role="alert">{saveError}</div>}
  </div>;
}
