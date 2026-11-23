import { t, tf, ctf, dateLocale, sortLocale } from '../../shared/i18n/locale';
import { LocalChatAvatar } from './LocalChatAvatar';
import { LoadingSkeleton } from '../../shared/ui/LoadingSkeleton';
import { ChatPlatformBar } from './ChatPlatformBar';
import { syncErrorMessage } from './chatPresentation';
import { ChatEventsPane, type EventItem } from './ChatEventsPane';
import { SidebarResizeHandle } from '../../shared/ui/SidebarResizeHandle';
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from '../../shared/api/desktopInvoke';
import { EvidenceDrawer, type EvidenceTarget } from '../../shared/ui/EvidenceDrawer';
import WeChatLocal from './local-reader/WeChatLocal';
import { ConversationSummary } from './summary/ConversationSummary';
import type { SummaryMessage } from './summary/summaryInput';
import { WhitelistSettings } from '../settings/Settings';
import wechatIcon from '../../assets/connectors/wechat.svg';
import whatsappIcon from '../../assets/connectors/whatsapp.svg';
import { syncFeedback, type SyncBatchStatus } from '../backup/syncFeedback';
import { requestFeedback } from '../../shared/api/desktopInvoke';
import { currentAccount as pickAccount, type ChatAccount } from '../connectors/connectorDetect';
import { useTaskRuntime } from '../tasks/taskRuntime';

type Session = { username: string; display: string; is_group: boolean; last_time: number | null; preview?: string | null; unread?: number | null; member_count?: number | null; community?: { id: string; name: string; is_parent: boolean } | null };
type OwnedSession = Session & { accountId: string; source: string };
type Settings = { sync?: { enabled?: boolean }; backup?: { mode?: string }; active_account_id?: string; connectors?: { wechat_enabled?: boolean; whatsapp_enabled?: boolean }; chat?: { sync_interval_seconds?: number; allowed_usernames?: Record<string, string[]> }; devify?: { active_space_id?: string | null; base_url?: string }; upload_queue?: Array<{ status?: string }> };
type SyncResult = { uploaded: number; message_count: number; batch_ids?: number[]; error?: string | null };
type Page<T> = { list?: T[]; pagination?: { total?: number; next?: unknown } };
type CloudConversation = { id: number; platform: string; source_namespace: string; source_conversation_id: string };
type Avatar = { username: string; data_url: string };
// Rust decides this from the newest local message and the newest uploaded position, so a
// failed upload or new local records cannot be hidden behind a queue status.
type UploadState = { username: string; state: 'pending' | 'local' | 'synced' | 'processing' | 'failed' | 'history_failed' | 'unknown'; new_messages: number };

type View = 'events' | 'raw' | 'summary';
type Filter = 'all' | 'pending';
type PlatformMemory = { session: OwnedSession | null; scrollTop: number; query: string; filter: Filter };
const needsBackup = (item: UploadState) => ['pending', 'local', 'failed', 'history_failed'].includes(item.state);
const sessionKey = (accountId: string, username: string) => JSON.stringify([accountId, username]);

const timeLabel = (value?: number | null) => {
  if (!value) return '';
  const date = new Date(value * 1000);
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  if (sameDay) return date.toLocaleTimeString(dateLocale(), { hour: '2-digit', minute: '2-digit' });
  return date.toLocaleDateString(dateLocale(), { month: '2-digit', day: '2-digit' });
};

export function ChatWorkbench({ visible = true, startFirstBackup, onFirstBackupStarted, onOpenData, onOpenWhitelist, onNewUpload, onOpenTasks, onSchedule, onOpenAccount, onOpenEvent, onAddPlatform }: { visible?: boolean; startFirstBackup?: boolean; onFirstBackupStarted?: () => void; onOpenData: () => void; onOpenWhitelist: () => void; onNewUpload: () => void; onOpenTasks: () => void; onSchedule: () => void; onOpenAccount: () => void; onOpenEvent?: (eventId: number) => void; onAddPlatform?: () => void }) {
  const runtime = useTaskRuntime();
  const configuredKey = JSON.stringify(runtime.sources?.filter(source => source.setup_complete).map(source => source.account_id).sort() ?? null);
  const [view, setView] = useState<View>('raw');
  const [addingSession, setAddingSession] = useState(false);
  const [sessionRevision, setSessionRevision] = useState(0);
  const [filter, setFilter] = useState<Filter>('all');
  const [connectedAccounts, setConnectedAccounts] = useState<ChatAccount[]>([]);
  const [activeSource, setActiveSource] = useState('');
  const activeSourceRef = useRef(activeSource);
  activeSourceRef.current = activeSource;
  const platformMemory = useRef(new Map<string, PlatformMemory>());
  const sessionList = useRef<HTMLDivElement>(null);
  const pageAnchor = useRef<{ source: string; key: string; top: number } | null>(null);
  const [seenActivity, setSeenActivity] = useState<Record<string, number>>({});
  const [query, setQuery] = useState('');
  const [detailQueries, setDetailQueries] = useState<Partial<Record<View, string>>>({});
  const detailQuery = detailQueries[view] ?? '';
  const [sessions, setSessions] = useState<OwnedSession[]>([]);
  const [selectedSession, setSelectedSession] = useState<OwnedSession | null>(null);
  useEffect(() => { setDetailQueries({}); }, [selectedSession?.accountId, selectedSession?.username]);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [eventsScope, setEventsScope] = useState('');
  const [summaryStatus, setSummaryStatus] = useState({count:0,busy:false});
  const [summarySelection,setSummarySelection] = useState<SummaryMessage[]>([]);
  const [summaryPickRequest,setSummaryPickRequest] = useState(0);
  const [settings, setSettings] = useState<Settings | null>(null);
  const accountId = selectedSession?.accountId ?? null;
  const sourceLabel = selectedSession?.source === 'whatsapp' ? 'WhatsApp' : t("微信");
  const [uploadStates, setUploadStates] = useState<Record<string, UploadState>>({});
  const [syncing, setSyncing] = useState(false);
  const [syncNotice, setSyncNotice] = useState('');
  const [syncNoticeError, setSyncNoticeError] = useState(false);
  const [syncScopeConfirmation, setSyncScopeConfirmation] = useState<string | null>(null);
  const [syncNoticePending, setSyncNoticePending] = useState(false);
  const [syncToastVisible, setSyncToastVisible] = useState(false);
  const [syncBatchIds, setSyncBatchIds] = useState<number[]>([]);
  const [loading, setLoading] = useState(true);
  const [eventsLoading, setEventsLoading] = useState(false);
  const [eventRefresh, setEventRefresh] = useState(0);
  const [error, setError] = useState('');
  const [eventsError, setEventsError] = useState('');
  const [rawFocus, setRawFocus] = useState<{ conversationId: string; messageId?: string | number } | null>(null);
  const [rawMounted, setRawMounted] = useState(false);
  const loadGeneration = useRef(0);
  // The raw pane reports loaded and selected messages to the conversation header.
  const [chatState, setChatState] = useState({ active: false, messages: 0, selected: 0, agentReady: false, agentName: '', agentModel: '' });
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);
  // Read messages only for the selected conversation, including the initial first chat.
  useEffect(() => { if (selectedSession && view === 'raw') setRawMounted(true); }, [selectedSession, view]);
  const [hasMoreSessions, setHasMoreSessions] = useState(false);
  const [moreSessionsErrors, setMoreSessionsErrors] = useState<Record<string, string>>({});
  const [loadingSource, setLoadingSource] = useState<string | null>(null);
  const sessionOffsets = useRef(new Map<string, { account: ChatAccount; offset: number }>());
  const moreLock = useRef(false);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    const configured: string[] | null = JSON.parse(configuredKey);
    if (!configured) return;
    const generation = ++loadGeneration.current;
    setLoading(true); setError(''); setMoreSessionsErrors({}); setLoadingSource(null);
    try {
      const [stored, found] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<ChatAccount[]>('chat_accounts')]);
      if (generation !== loadGeneration.current) return;
      const connections = stored.connectors ?? {};
      const accounts = (found ?? []).filter(account => configured.includes(account.id)).filter(account => account.source === 'whatsapp'
        ? connections.whatsapp_enabled !== false
        : connections.wechat_enabled !== false);

      const results = await Promise.allSettled(accounts.map(async item => ({ account: item, sessions: await invoke<Session[]>('chat_sessions', { accountId: item.id, limit: 50 }) })));
      if (generation !== loadGeneration.current) return;
      const list = results.flatMap(result => result.status === 'fulfilled'
        ? (result.value.sessions ?? []).map(session => ({ ...session, accountId: result.value.account.id, source: result.value.account.source ?? 'wechat' })) : []);
      list.sort((a, b) => (b.last_time ?? 0) - (a.last_time ?? 0));
      const failed = results.flatMap((result, index) => result.status === 'rejected' ? [accounts[index].source === 'whatsapp' ? 'WhatsApp' : t("微信")] : []);
      setSettings(stored);
      setConnectedAccounts(accounts);
      const sourceCounts = new Map<string, number>();
      setSessions(list.filter(item => {
        const count = sourceCounts.get(item.source) ?? 0;
        sourceCounts.set(item.source, count + 1);
        return count < 200;
      }));
      const available = [...new Set(accounts.map(item => item.source ?? 'wechat'))];
      const source = available.includes(activeSourceRef.current) ? activeSourceRef.current : list[0]?.source ?? available[0] ?? '';
      activeSourceRef.current = source;
      setActiveSource(source);
      sessionOffsets.current = new Map(results.flatMap(result => result.status === 'fulfilled' && result.value.sessions.length === 50 ? [[result.value.account.id, { account: result.value.account, offset: 50 }] as const] : []));
      setHasMoreSessions(sessionOffsets.current.size > 0);
      setSelectedSession(current => {
        const remembered = current?.source === source ? current : platformMemory.current.get(source)?.session;
        // A refreshed first page does not prove a previously selected chat was deleted.
        return list.find(item => item.accountId === remembered?.accountId && item.username === remembered?.username)
          ?? (remembered && accounts.some(account => account.id === remembered.accountId)
            && stored.chat?.allowed_usernames?.[remembered.accountId]?.includes(remembered.username)
            ? remembered : list.find(item => item.source === source) ?? null);
      });
      if (failed.length) setError(tf("{0}会话读取失败，请刷新重试。", [...new Set(failed)].join('、')));
      setEventRefresh(value => value + 1);

    } catch (reason) { if (generation === loadGeneration.current) setError(String(reason)); }
    finally { if (generation === loadGeneration.current) setLoading(false); }
  }, [configuredKey]);
  useEffect(() => {
    const configured: string[] = JSON.parse(configuredKey) ?? [];
    setConnectedAccounts(current => current.filter(account => configured.includes(account.id)));
    setSessions(current => current.filter(session => configured.includes(session.accountId)));
    setSelectedSession(current => current && configured.includes(current.accountId) ? current : null);
    for (const id of sessionOffsets.current.keys()) if (!configured.includes(id)) sessionOffsets.current.delete(id);
    if (visible) void load();
    return () => { loadGeneration.current += 1; };
  }, [load, visible]);
  const loadMoreSessions = async (source: string) => {
    if (moreLock.current) return;
    moreLock.current = true; setLoading(true); setMoreSessionsErrors(current => ({ ...current, [source]: '' })); setLoadingSource(source);
    const generation = loadGeneration.current;
    try {
      // Page only this platform, reading at most 50 rows per account in sequence.
      const added: OwnedSession[] = [];
      const pages: Array<{ id: string; entry: { account: ChatAccount; offset: number }; count: number }> = [];
      for (const [id, entry] of sessionOffsets.current) {
        if ((entry.account.source ?? 'wechat') !== source) continue;
        const rows = await invoke<Session[]>('chat_sessions', { accountId: id, limit: 50, offset: entry.offset });
        if (generation !== loadGeneration.current) return;
        added.push(...rows.map(row => ({ ...row, accountId: id, source: entry.account.source ?? 'wechat' })));
        pages.push({ id, entry, count: rows.length });
      }
      // Commit offsets only after the whole page succeeds, so a retry cannot skip rows.
      for (const { id, entry, count } of pages) {
        if (count < 50) sessionOffsets.current.delete(id); else entry.offset += 50;
      }
      if (activeSourceRef.current === source && sessionList.current) {
        const top = sessionList.current.getBoundingClientRect().top;
        const anchor = [...sessionList.current.querySelectorAll<HTMLElement>('[data-session-key]')].find(node => node.getBoundingClientRect().bottom > top);
        if (anchor) pageAnchor.current = { source, key: anchor.dataset.sessionKey!, top: anchor.getBoundingClientRect().top };
      }
      setSessions(current => {
        const merged = [...new Map([...current, ...added].map(row => [sessionKey(row.accountId, row.username), row])).values()];
        // Keep each platform's paging window independent; loading one never evicts
        // another platform's rows while leaving its offset advanced.
        const other = merged.filter(row => row.source !== source);
        const own = merged.filter(row => row.source === source).slice(-200);
        return [...other, ...own];
      });
      setHasMoreSessions(sessionOffsets.current.size > 0);
    } catch (reason) { if (generation === loadGeneration.current) setMoreSessionsErrors(current => ({ ...current, [source]: String(reason) })); }
    finally { moreLock.current = false; if (generation === loadGeneration.current) { setLoading(false); setLoadingSource(null); } }
  };

  useEffect(() => {
    let cancelled = false;
    const spaceId = settings?.devify?.active_space_id;
    const username = selectedSession?.username;
    const requestScope = JSON.stringify([settings?.devify?.base_url, spaceId, accountId, username]);
    setEventsError('');
    if (!isTauri() || !visible || view !== 'events' || !spaceId || !username || !accountId) {
      setEventsLoading(false);
      return () => { cancelled = true; };
    }
    setEventsLoading(true);
    void (async () => {
      try {
        const found = await invoke<{ conversation: { id: number } | null }>('chat_remote_lookup', { accountId, username });
        if (cancelled) return;
        const conversation = found.conversation;
        if (!conversation) { setEventsScope(requestScope); setEvents([]); return; }
        const result = await invoke<Page<EventItem>>('devify_analysis_list', { kind: 'events', spaceId, page: 1, conversationId: conversation.id });
        if (!cancelled) { setEventsScope(requestScope); setEvents(result.list ?? []); }
      } catch (reason) {
        if (!cancelled) setEventsError(String(reason));
      } finally {
        if (!cancelled) setEventsLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [settings?.devify?.base_url, settings?.devify?.active_space_id, accountId, selectedSession?.username, eventRefresh, visible, view]);

  useEffect(() => {
    if (!eventsError) return;
    const retry = () => setEventRefresh(value => value + 1);
    const timer = window.setTimeout(retry, 30000);
    window.addEventListener('online', retry);
    return () => { window.clearTimeout(timer); window.removeEventListener('online', retry); };
  }, [eventsError]);

  useEffect(()=>{setSummarySelection([]);setSummaryPickRequest(0);},[accountId]);

  const platformSessions = useMemo(() => sessions.filter(session => session.source === activeSource), [sessions, activeSource]);
  useEffect(() => {
    const anchor = pageAnchor.current;
    pageAnchor.current = null;
    if (!anchor || anchor.source !== activeSource || !sessionList.current) return;
    const node = [...sessionList.current.querySelectorAll<HTMLElement>('[data-session-key]')].find(item => item.dataset.sessionKey === anchor.key);
    if (node) sessionList.current.scrollTop += node.getBoundingClientRect().top - anchor.top;
  }, [sessions, activeSource]);
  const pendingSessions = useMemo(() => new Set(platformSessions.filter(session => {
    const state = uploadStates[sessionKey(session.accountId, session.username)];
    return state && needsBackup(state);
  }).map(session => sessionKey(session.accountId, session.username))), [platformSessions, uploadStates]);
  const pendingCount = pendingSessions.size;
  const activeUploads = useMemo(() => (settings?.upload_queue ?? []).filter(job => job.status != null && ['queued', 'running', 'processing', 'uploading'].includes(job.status)), [settings]);

  useEffect(() => { setUploadStates({}); }, [settings?.devify?.base_url, settings?.devify?.active_space_id]);

  // Read badges only for the displayed platform. A slow scan cannot overlap another
  // scan or commit after a platform/account/space change.
  const uploadStateReading = useRef(false);
  const [badgeRetry, setBadgeRetry] = useState(0);
  const uploadStateScope = JSON.stringify([visible, settings?.devify?.base_url, settings?.devify?.active_space_id, activeSource, platformSessions.map(item => sessionKey(item.accountId, item.username))]);
  const currentUploadStateScope = useRef(uploadStateScope);
  currentUploadStateScope.current = uploadStateScope;
  const refreshUploadState = useCallback(async () => {
    if (!visible || !isTauri() || !platformSessions.length || uploadStateReading.current) return;
    uploadStateReading.current = true;
    const scope = uploadStateScope;
    try {
      const next: Record<string, UploadState> = {};
      for (const id of new Set(platformSessions.map(item => item.accountId))) {
        const usernames = platformSessions.filter(session => session.accountId === id).map(session => session.username);
        const items = await invoke<UploadState[]>('wechat_upload_state', { accountId: id, usernames });
        if (currentUploadStateScope.current !== scope) return;
        for (const item of items) next[sessionKey(id, item.username)] = item;
      }
      if (currentUploadStateScope.current === scope) setUploadStates(current => {
        const retained = new Set(sessions.map(session => sessionKey(session.accountId, session.username)));
        const merged = Object.fromEntries(Object.entries(current).filter(([key]) => retained.has(key)));
        for (const session of platformSessions) delete merged[sessionKey(session.accountId, session.username)];
        return { ...merged, ...next };
      });
    } catch { /* retain last-confirmed badges on read failure */ }
    finally {
      uploadStateReading.current = false;
      // A new platform may have become visible while this scan was in flight.
      if (currentUploadStateScope.current !== scope) setBadgeRetry(value => value + 1);
    }
  }, [visible, sessions, platformSessions, uploadStateScope]);
  useEffect(() => {
    if (!visible) return;
    void refreshUploadState();
    const timer = window.setInterval(() => void refreshUploadState(), activeUploads.length || syncNoticePending ? 5000 : 60000);
    return () => window.clearInterval(timer);
  }, [visible, refreshUploadState, activeUploads.length, syncNoticePending, badgeRetry]);

  const selectPlatform = (source: string) => {
    if (source === activeSource) return;
    platformMemory.current.set(activeSource, { session: selectedSession, scrollTop: sessionList.current?.scrollTop ?? 0, query, filter });
    const previous = platformMemory.current.get(source);
    const remembered = previous?.session;
    activeSourceRef.current = source;
    setActiveSource(source);
    setSelectedSession(remembered && connectedAccounts.some(account => account.id === remembered.accountId)
      && settings?.chat?.allowed_usernames?.[remembered.accountId]?.includes(remembered.username)
      ? sessions.find(item => item.accountId === remembered.accountId && item.username === remembered.username) ?? remembered
      : sessions.find(item => item.source === source) ?? null);
    setQuery(previous?.query ?? ''); setFilter(previous?.filter ?? 'all'); setRawFocus(null);
  };
  useEffect(() => {
    if (sessionList.current) sessionList.current.scrollTop = platformMemory.current.get(activeSource)?.scrollTop ?? 0;
  }, [activeSource]);
  const latestActivity = Math.max(0, ...platformSessions.map(session => session.last_time ?? 0));
  useEffect(() => {
    if (visible && activeSource) setSeenActivity(current => (current[activeSource] ?? 0) >= latestActivity ? current : { ...current, [activeSource]: latestActivity });
  }, [activeSource, latestActivity, visible]);
  const wechatLabel = t('微信');
  const platforms = useMemo(() => [...new Set(connectedAccounts.map(account => account.source ?? 'wechat'))].sort().map(source => ({
    id: source, label: source === 'whatsapp' ? 'WhatsApp' : wechatLabel, icon: source === 'whatsapp' ? whatsappIcon : wechatIcon,
    updated: source !== activeSource && sessions.some(session => session.source === source && (session.last_time ?? 0) > (seenActivity[source] ?? 0)),
  })), [connectedAccounts, activeSource, sessions, seenActivity, wechatLabel]);

  const syncNow = async (scopeConfirmation?: string) => {
    if (syncing) return;
    setSyncScopeConfirmation(null); setSyncBatchIds([]); setSyncing(true); setSyncNotice(''); setSyncNoticeError(false); setSyncNoticePending(false); setSyncToastVisible(true);
    try {
      const result = await invoke<SyncResult>('devify_sync_now', { scopeConfirmation });
      setSyncNoticeError(Boolean(result.error));
      const ids = result.batch_ids ?? [];
      setSyncBatchIds(result.error ? [] : ids);
      setSyncNoticePending(!result.error && ids.length > 0);
      setSyncNotice(result.error
        ? (result.message_count ? tf("已提交 {0} 条记录，部分会话未完成。 ", result.message_count) : t("本次未提交记录。")) + syncErrorMessage(result.error)
        : result.message_count
          ? ids.length ? tf("已提交 {0} 条记录，等待服务端处理。", result.message_count) : tf("已提交 {0} 条记录，请在任务中查看状态。", result.message_count)
          : t("没有需要提交的新记录。"));
      await refreshUploadState();
    } catch (reason) {
      const confirmation = /^SYNC_SCOPE_CONFIRM:([a-f0-9]{64})$/.exec(String(reason));
      setSyncNoticeError(!String(reason).includes(t("该账号正在上传")));
      if (confirmation) {
        setSyncScopeConfirmation(confirmation[1]);
        setSyncNotice(t("当前账号和空间没有可用的同步进度。请先检查首次上传范围，再开始同步。"));
      } else {
        setSyncNotice(syncErrorMessage(reason));
      }
    }
    finally { setSyncing(false); }
  };

  const firstBackupStarted = useRef(false);
  useEffect(() => {
    if (!startFirstBackup || firstBackupStarted.current) return;
    firstBackupStarted.current = true;
    onFirstBackupStarted?.();
    void syncNow();
  }, [startFirstBackup]);

  useEffect(() => {
    if (!syncBatchIds.length) return;
    let active = true;
    let timer: number | undefined;
    const poll = async () => {
      try {
        const batches = await Promise.all(syncBatchIds.map(batchId => invoke<SyncBatchStatus>('devify_upload_status', { batchId })));
        if (!active) return;
        const status = syncFeedback(batches);
        if (status === 'processing') { timer = window.setTimeout(() => void poll(), 5000); return; }
        setSyncBatchIds([]); setSyncNoticePending(false);
        if (status === 'completed') {
          setSyncNotice(t("服务端已完成处理。"));
          void refreshUploadState();
        } else {
          setSyncNoticeError(true);
          const failed = batches.find(batch => ['failed', 'cancelled'].includes(batch.status));
          setSyncNotice(status === 'unknown' ? t("暂时无法确认服务端处理状态，请查看任务。") : failed?.error_message ? tf("服务端处理未完成：{0}", failed.error_message) : t("服务端处理未完成，请查看任务。"));
        }
      } catch {
        if (!active) return;
        setSyncBatchIds([]); setSyncNoticePending(false); setSyncNoticeError(true);
        setSyncNotice(t("暂时无法确认服务端处理状态，请查看任务。"));
      }
    };
    void poll();
    return () => { active = false; if (timer !== undefined) window.clearTimeout(timer); };
  }, [syncBatchIds, refreshUploadState]);

  useEffect(() => {
    if (syncing || syncNoticePending || syncNoticeError || !syncNotice) return;
    const timer = window.setTimeout(() => { setSyncToastVisible(false); setSyncNotice(''); }, 4000);
    return () => window.clearTimeout(timer);
  }, [syncing, syncNoticePending, syncNoticeError, syncNotice]);

  const visibleSessions = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return platformSessions.filter(session => (!keyword || `${session.display} ${session.username}`.toLowerCase().includes(keyword)) && (filter === 'all' || pendingSessions.has(sessionKey(session.accountId, session.username))));
  }, [platformSessions, query, filter, pendingSessions]);
  /** Community grouping inside one source; the newest-activity order is preserved. */
  const groupSessions = (list: OwnedSession[]) => {
    const groups: Array<{ key: string; name?: string; sessions: OwnedSession[] }> = [];
    const communities = new Map<string, { key: string; name?: string; sessions: OwnedSession[] }>();
    for (const session of list) {
      if (!session.community) {
        groups.push({ key: `session:${sessionKey(session.accountId, session.username)}`, sessions: [session] });
        continue;
      }
      const communityKey = sessionKey(session.accountId, session.community.id);
      let group = communities.get(communityKey);
      if (!group) {
        group = { key: `community:${communityKey}`, name: session.community.name, sessions: [] };
        communities.set(communityKey, group);
        groups.push(group);
      }
      if (session.community.is_parent) group.sessions.unshift(session);
      else group.sessions.push(session);
    }
    return groups;
  };
  const scopedEvents = eventsScope === JSON.stringify([settings?.devify?.base_url, settings?.devify?.active_space_id, accountId, selectedSession?.username]) ? events : [];

  const openEventPage = (eventId: number) => {
    if (onOpenEvent) onOpenEvent(eventId);
    else { setView('events'); }
  };

  const openRaw = (username: string) => {
    const next = sessions.find(item => item.accountId === accountId && item.username === username);
    if (next) setSelectedSession(next);
    setRawFocus({ conversationId: username });
    setView('raw');
  };

  if (!isTauri()) return null;

  const conversation = selectedSession?.display ?? platforms.find(platform => platform.id === activeSource)?.label ?? t("聊天");
  const sessionMeta = (session: OwnedSession) => {
    const state = uploadStates[sessionKey(session.accountId, session.username)];
    if (state?.state === 'pending') return { text: state.new_messages ? tf("本机新增待上传 · {0} 条", state.new_messages) : t("本机新增待上传"), tone: 'pending' };
    if (state?.state === 'local') return { text: t("待首次备份"), tone: 'pending' };
    if (state?.state === 'processing') return { text: t("已提交 · 等待服务端处理"), tone: 'pending' };
    if (state?.state === 'failed') return { text: t("上传失败 · 请检查任务"), tone: 'pending' };
    if (state?.state === 'history_failed') return { text: t("历史任务未完成 · 请检查任务"), tone: 'pending' };
    if (state?.state === 'synced') return { text: t("已同步 · 最近确认"), tone: 'synced' };
    return { text: t("同步状态待确认"), tone: 'local' };
  };

  return <div className="chat-workbench">
    <aside className="chat-workbench-sidebar">
      <header className="chat-workbench-sidebar-head">
        <h1>{t("聊天")}</h1>
        <div className="chat-workbench-header-actions"><button type="button" className="chat-workbench-add" onClick={() => setAddingSession(true)} aria-label={t("添加会话")} title={t("添加会话")}><span aria-hidden="true">＋</span></button></div>
        <p>{loading && !sessions.length ? t("正在读取会话…") : tf("{0} 个会话 · {1} 个待传", platformSessions.length, pendingCount)}</p>
      </header>
      <ChatPlatformBar platforms={platforms} selected={activeSource} onSelect={selectPlatform} onAddPlatform={onAddPlatform} visible={visible}/>
      <input className="chat-workbench-search" value={query} onChange={event => setQuery(event.target.value)} placeholder={t("搜索会话")}/>
      <div className="chat-workbench-filters">
        <button type="button" className={filter === 'all' ? 'active' : ''} onClick={() => setFilter('all')}>{t("全部")} <small>{platformSessions.length}</small></button>
        <button type="button" className={filter === 'pending' ? 'active' : ''} onClick={() => setFilter('pending')}>{t("待传")} <small>{pendingCount}</small></button>
      </div>
      <div className="chat-workbench-session-label"><span>{t("会话 ·")} {visibleSessions.length}</span><span className="chat-workbench-list-tools">{t("按最后更新")} <button type="button" className="chat-workbench-refresh" onClick={() => void load()} aria-label={t("刷新会话")} title={t("刷新会话")}>↻</button></span></div>
      <div className="chat-workbench-sessions" ref={sessionList} aria-busy={loading} onScroll={event => {
        const list = event.currentTarget;
        if (!visible || !hasMoreSessions || loading) return;
        const viewport = list.getBoundingClientRect();
        for (const marker of list.querySelectorAll<HTMLElement>('[data-more-source]')) {
          const source = marker.dataset.moreSource!;
          const bounds = marker.getBoundingClientRect();
          if (source === activeSource && !moreSessionsErrors[source] && bounds.top <= viewport.bottom + 160 && bounds.bottom >= viewport.top) { void loadMoreSessions(source); break; }
        }
      }}>
        {loading && !sessions.length && <LoadingSkeleton label={t("正在加载会话")} />}
        {error && <p className="chat-workbench-load-error" role="alert">{t(error)} <button type="button" onClick={() => void load()}>{t("重试")}</button></p>}
        <div className="chat-workbench-source-section" key={activeSource}>
            {groupSessions(visibleSessions).map(item => <Fragment key={item.key}>
              {item.name && <div className="chat-workbench-community-label">{item.name} {t("· 社区")}</div>}
              {item.sessions.map(session => {
                const meta = sessionMeta(session);
                return <button type="button" key={sessionKey(session.accountId, session.username)} data-session-key={sessionKey(session.accountId, session.username)} className={`chat-workbench-session${selectedSession?.accountId === session.accountId && selectedSession?.username === session.username ? ' selected' : ''}`} onClick={() => { setRawFocus(null); setSelectedSession(session); }}>
                  <span className="chat-workbench-avatar">
                    <LocalChatAvatar key={sessionKey(session.accountId, session.username)} accountId={session.accountId} username={session.username} label={session.display} />
                    <i className="chat-workbench-avatar-badge" aria-hidden="true"><img src={session.source === 'whatsapp' ? whatsappIcon : wechatIcon} alt="" /></i>
                  </span>
                  <span className="chat-workbench-session-body">
                    <span className="chat-workbench-session-line"><strong>{session.community?.is_parent ? t("社区") : session.display}</strong><time>{timeLabel(session.last_time)}</time></span>
                    <small>{session.preview ? (session.preview.length > 34 ? `${session.preview.slice(0, 34)}…` : session.preview) : t("暂无消息预览")}</small>
                    <em className={meta.tone}>{meta.text}</em>
                  </span>
                </button>;
              })}
            </Fragment>)}
            {[...sessionOffsets.current.values()].some(entry => (entry.account.source ?? 'wechat') === activeSource) && <div className="chat-workbench-source-more" data-more-source={activeSource}>
              {loadingSource === activeSource && <p className="chat-workbench-empty" role="status">{t('正在读取会话…')}</p>}
              {moreSessionsErrors[activeSource] && <p className="chat-workbench-load-error" role="alert">{t(moreSessionsErrors[activeSource])} <button type="button" disabled={loading} onClick={() => void loadMoreSessions(activeSource)}>{t('重试')}</button></p>}
              {!loading && !moreSessionsErrors[activeSource] && (query.trim() || filter !== 'all') && <button type="button" className="connector-quiet" onClick={() => void loadMoreSessions(activeSource)}>{t('加载更多会话')}</button>}
            </div>}
          </div>
        {!loading && !visibleSessions.length && <p className="chat-workbench-empty">{t("没有符合条件的会话。")}</p>}
      </div>
      <SidebarResizeHandle/>
    </aside>

    <main className="chat-workbench-main">
      <header className="chat-workbench-head">
        <div className="chat-workbench-title">
          {selectedSession && <span className="chat-workbench-source-icon" title={sourceLabel}><img src={selectedSession.source==='whatsapp'?whatsappIcon:wechatIcon} alt={sourceLabel}/></span>}
          <div className="chat-workbench-title-text"><h2 title={conversation}>{conversation}</h2></div>
        </div>
        <div className="chat-workbench-head-right">
          <div className="chat-workbench-tabs" role="tablist" aria-label={t("聊天视图")}>
            <button type="button" role="tab" aria-selected={view === 'raw'} className={view === 'raw' ? 'active' : ''} onClick={() => setView('raw')}>{t("原始记录")}{view === 'raw' && chatState.active && <small>{chatState.messages}</small>}</button>
            <button type="button" role="tab" aria-selected={view === 'events'} className={view === 'events' ? 'active' : ''} onClick={() => setView('events')}>{t("待办")} <small>{scopedEvents.length}</small></button>
            <button type="button" role="tab" aria-selected={view === 'summary'} className={view === 'summary' ? 'active' : ''} onClick={() => setView('summary')}>
              {t("聊天总结")}
              {summaryStatus.busy
                ? <i className="chat-workbench-tab-spinner" aria-label={t("正在生成")}/>
                : summaryStatus.count > 0 && <small>{summaryStatus.count}</small>}
            </button>
          </div>
          <div className="chat-workbench-head-tools"><input className="chat-workbench-head-search" aria-label={view === 'raw' ? t("在此会话中搜索") : view === 'summary' ? t("搜索聊天总结") : t("搜索待办")} value={detailQuery} onChange={event => { const value = event.target.value; setDetailQueries(current => ({ ...current, [view]: value })); }} placeholder={view === 'raw' ? t("在此会话中搜索") : view === 'summary' ? t("搜索聊天总结") : t("搜索待办")}/>
          </div>
        </div>
        {selectedSession && <p className="chat-workbench-conversation-meta"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">{selectedSession.is_group?<><circle cx="9" cy="8" r="3"/><path d="M3 20v-2a6 6 0 0 1 12 0v2M16 5a3 3 0 0 1 0 6M21 20v-2a6 6 0 0 0-3-5"/></>:<><circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/></>}</svg>{selectedSession.is_group ? t("群聊") : t("单聊")}{selectedSession.is_group && selectedSession.member_count != null ? tf(" · {0} 人", selectedSession.member_count) : ''}{view === 'raw' && chatState.active ? ` · ${tf('已加载 {0} 条消息', chatState.messages)}` : ''}</p>}
      </header>

      <ChatEventsPane
        active={view === 'events'}
        loading={eventsLoading}
        error={error || eventsError}
        events={scopedEvents}
        sessionCount={sessions.length}
        query={detailQueries.events ?? ''}
        filter={filter}
        onRetry={() => { if (error) void load(); else setEventRefresh(value => value + 1); }}
        onSchedule={onSchedule}
        onOpenEvidence={event => setEvidence({ kind: 'event', id: event.id, title: event.title })}
      />

      {(!rawMounted || !accountId) && view === 'raw' && <div className="chat-workbench-empty" role="status">{platformSessions.length ? t("选择一个会话开始查看") : t("该平台暂无已导入的会话。")}</div>}
      {rawMounted && accountId && <div className={`chat-workbench-pane chat-workbench-raw-content chat-workbench-embedded${view === 'raw' ? '' : ' is-hidden'}`}>
        {syncToastVisible && (syncing || syncNotice) && <div className={`sync-feedback-toast${syncNoticeError ? ' is-error' : ''}${syncScopeConfirmation ? ' is-confirmation' : ''}`} role={syncNoticeError ? 'alert' : 'status'}>
          <span className={`sync-feedback-icon${syncing || syncNoticePending ? ' is-loading' : ''}`} aria-hidden="true">{syncing || syncNoticePending ? '' : syncNoticeError ? '!' : '✓'}</span>
          <span className="sync-feedback-text">{syncing ? t("正在提交记录…") : syncNotice}</span>
          {syncScopeConfirmation ? <div className="sync-feedback-actions">
            <button type="button" onClick={onOpenWhitelist}>{t("检查上传范围")}</button>
            <button type="button" onClick={() => void syncNow(syncScopeConfirmation)}>{t("确认并开始同步")}</button>
          </div> : <>
            {!syncing && (syncNoticeError || syncNotice.includes(t("后台上传"))) && <button type="button" onClick={onOpenTasks}>{t("查看任务")}</button>}
            {syncNoticeError && <button type="button" onClick={() => requestFeedback('upload', 'chat', syncBatchIds.slice(0, 20))}>{t("反馈此问题")}</button>}
          </>}
          <button type="button" className="sync-feedback-close" onClick={() => setSyncToastVisible(false)} aria-label={t("关闭提示")}>×</button>
        </div>}
        {accountId && <WeChatLocal
          key={accountId}
          accountId={accountId}
          accountDisplay={connectedAccounts.find(account => account.id === accountId)?.display}
          sessionRevision={sessionRevision}
          focusConversation={rawFocus?.conversationId ?? selectedSession?.username}
          focusMessageId={rawFocus?.messageId}
          searchKeyword={view === 'raw' ? detailQuery : ''}
          onClearSearch={() => setQuery('')}
          onChatState={setChatState}
          onSummarySelection={setSummarySelection}
          onSummarizeSelection={()=>{setSummaryPickRequest(value=>value+1);setView('summary');}}
          onOpenData={onOpenData}
          onOpenWhitelist={onOpenWhitelist}
        />}
      </div>}

      <div className={`chat-workbench-pane${view === 'summary' ? '' : ' is-hidden'}`}>
        <ConversationSummary
          visible={visible&&view==='summary'}
          localAgentReady={chatState.agentReady}
          localAgentName={chatState.agentName}
          localAgentModel={chatState.agentModel}
          spaceId={settings?.devify?.active_space_id??null}
          accountId={accountId}
          sessions={sessions.filter(session => session.accountId === accountId)}
          session={selectedSession}
          selection={summarySelection}
          selectionTotal={chatState.selected}
          pickRequest={summaryPickRequest}
          query={detailQuery}
          onPick={()=>setView('raw')}
          onOpenTasks={onOpenTasks}
          onStatus={setSummaryStatus}
        />
      </div>
    </main>

    {addingSession && <div className="chat-add-overlay" role="presentation"><section className="chat-add-dialog" role="dialog" aria-modal="true" aria-label={t("添加会话")}><button type="button" className="chat-add-close" onClick={() => setAddingSession(false)} aria-label={t("关闭")}>×</button><WhitelistSettings onSaved={() => { setAddingSession(false); setSessionRevision(value => value + 1); void load(); }}/></section></div>}
    {evidence && <EvidenceDrawer
      spaceId={settings?.devify?.active_space_id ?? null}
      target={evidence}
      onClose={() => setEvidence(null)}
      onNavigate={setEvidence}
      onOpenDetail={target => { if (target.kind === 'event') openEventPage(target.id); }}
    />}
  </div>;
}
