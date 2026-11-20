import { t, tf, ctf, dateLocale, sortLocale } from './locale';
import { LocalChatAvatar } from './LocalChatAvatar';
import { LoadingSkeleton } from './LoadingSkeleton';
import { eventViewState, syncErrorMessage } from './chatPresentation';
import { SidebarResizeHandle } from './SidebarResizeHandle';
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from './desktopInvoke';
import { EvidenceDrawer, type EvidenceTarget } from './EvidenceDrawer';
import WeChatLocal from './WeChatLocal';
import { ChatSummary } from './ChatSummary';
import type { SummaryRecord } from './summary';
import { exportNotSupported } from './toast';
import { WhitelistSettings } from './Settings';
import wechatIcon from './assets/connectors/wechat.svg';
import whatsappIcon from './assets/connectors/whatsapp.svg';
import { syncFeedback, type SyncBatchStatus } from './syncFeedback';
import { requestFeedback } from './desktopInvoke';
import { useCapability, useInternalAccount } from './entitlements';
import { currentAccount as pickAccount, type ChatAccount } from './connectorDetect';

type Session = { username: string; display: string; is_group: boolean; last_time: number | null; preview?: string | null; unread?: number | null; member_count?: number | null; community?: { id: string; name: string; is_parent: boolean } | null };
type OwnedSession = Session & { accountId: string; source: string };
type Settings = { sync?: { enabled?: boolean }; backup?: { mode?: string }; active_account_id?: string; connectors?: { wechat_enabled?: boolean; whatsapp_enabled?: boolean }; chat?: { sync_interval_seconds?: number; allowed_usernames?: Record<string, string[]> }; devify?: { active_space_id?: string | null; base_url?: string }; upload_queue?: Array<{ status?: string }> };
type SyncResult = { uploaded: number; message_count: number; batch_ids?: number[]; error?: string | null };
type EventItem = { id: number; title: string; content?: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };
type Page<T> = { list?: T[]; pagination?: { total?: number; next?: unknown } };
type CloudConversation = { id: number; platform: string; source_namespace: string; source_conversation_id: string };
type Avatar = { username: string; data_url: string };
// Rust decides this from the newest local message and the newest uploaded position, so a
// failed upload or new local records cannot be hidden behind a queue status.
type UploadState = { username: string; state: 'pending' | 'local' | 'synced' | 'processing' | 'failed' | 'history_failed' | 'unknown'; new_messages: number };

type View = 'events' | 'raw' | 'summary';
type Filter = 'all' | 'pending' | 'local';
const sessionKey = (accountId: string, username: string) => JSON.stringify([accountId, username]);

const dateLabel = (value?: string | number | null) => value ? new Date(typeof value === 'number' ? value * 1000 : value).toLocaleString(dateLocale(), { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : t("时间未知");
const timeLabel = (value?: number | null) => {
  if (!value) return '';
  const date = new Date(value * 1000);
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  if (sameDay) return date.toLocaleTimeString(dateLocale(), { hour: '2-digit', minute: '2-digit' });
  return date.toLocaleDateString(dateLocale(), { month: '2-digit', day: '2-digit' });
};
const dayLabel = (value?: string | null) => {
  if (!value) return t("时间未知");
  const date = new Date(value);
  const now = new Date();
  const day = date.toLocaleDateString(dateLocale(), { month: '2-digit', day: '2-digit' });
  if (date.toDateString() === now.toDateString()) return tf("{0} · 今天", day);
  const yesterday = new Date(now.getTime() - 86400000);
  if (date.toDateString() === yesterday.toDateString()) return tf("{0} · 昨天", day);
  return day;
};
const statusLabel = (value?: string | null) => ({ open: t("待我处理"), in_progress: t("推进中"), done: t("已完成") } as Record<string, string>)[value || ''] || t("未标记");

function StateDot({ done }: { done: boolean }) {
  return <svg className="chat-event-dot" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25" fill={done ? '#1a1a19' : 'transparent'} stroke={done ? '#1a1a19' : '#c4a06a'} strokeWidth={1.5}/>
    {done && <path d="M5 8.3l2.1 2.1L11 6.1" stroke="#fff" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
  </svg>;
}

export function ChatWorkbench({ visible = true, startFirstBackup, onFirstBackupStarted, onOpenData, onOpenWhitelist, onNewUpload, onOpenTasks, onSchedule, onOpenAccount, onOpenEvent }: { visible?: boolean; startFirstBackup?: boolean; onFirstBackupStarted?: () => void; onOpenData: () => void; onOpenWhitelist: () => void; onNewUpload: () => void; onOpenTasks: () => void; onSchedule: () => void; onOpenAccount: () => void; onOpenEvent?: (eventId: number) => void }) {
  const [view, setView] = useState<View>('raw');
  // Local summaries are an internal-account feature: without the entitlement the summary tab
  // and its 整理工作内容 entry point disappear, leaving only records and cloud events.
  const { internal: internalAccount } = useInternalAccount();
  const { allowed: workDigestGranted } = useCapability('ai.work_digest');
  const canOrganize = internalAccount && workDigestGranted;
  useEffect(() => {
    if (!internalAccount) setView(current => (current === 'summary' ? 'raw' : current));
  }, [internalAccount]);
  const [addingSession, setAddingSession] = useState(false);
  const [sessionRevision, setSessionRevision] = useState(0);
  const [filter, setFilter] = useState<Filter>('all');
  const [collapsedSources, setCollapsedSources] = useState<Record<string, boolean>>({});
  const [query, setQuery] = useState('');
  const [sessions, setSessions] = useState<OwnedSession[]>([]);
  const [selectedSession, setSelectedSession] = useState<OwnedSession | null>(null);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [eventsScope, setEventsScope] = useState('');
  const [summaries, setSummaries] = useState<SummaryRecord[]>([]);
  const [summaryBusy, setSummaryBusy] = useState(false);
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
  const [openSummaryRequest, setOpenSummaryRequest] = useState(false);
  const [rawFocus, setRawFocus] = useState<{ conversationId: string; messageId?: string | number } | null>(null);
  const [rawMounted, setRawMounted] = useState(false);
  const loadGeneration = useRef(0);
  // The raw pane owns the message list, so it reports back whether 整理/导出 can act.
  const [chatState, setChatState] = useState({ active: false, messages: 0, selected: 0, agentReady: false });
  const [workRequest, setWorkRequest] = useState(0);
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);
  // Read messages only for the selected conversation, including the initial first chat.
  useEffect(() => { if (selectedSession && view === 'raw') setRawMounted(true); }, [selectedSession, view]);
  const [hasMoreSessions, setHasMoreSessions] = useState(false);
  const [moreSessionsError, setMoreSessionsError] = useState<{ source: string; message: string } | null>(null);
  const [loadingSource, setLoadingSource] = useState<string | null>(null);
  const sessionOffsets = useRef(new Map<string, { account: ChatAccount; offset: number }>());
  const moreLock = useRef(false);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    const generation = ++loadGeneration.current;
    setLoading(true); setError(''); setMoreSessionsError(null); setLoadingSource(null);
    try {
      const [stored, found] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<ChatAccount[]>('chat_accounts')]);
      const connections = stored.connectors ?? {};
      const accounts = (found ?? []).filter(account => account.source === 'whatsapp'
        ? connections.whatsapp_enabled !== false
        : connections.wechat_enabled !== false);

      const results = await Promise.allSettled(accounts.map(async item => ({ account: item, sessions: await invoke<Session[]>('chat_sessions', { accountId: item.id, limit: 50 }) })));
      if (generation !== loadGeneration.current) return;
      const list = results.flatMap(result => result.status === 'fulfilled'
        ? (result.value.sessions ?? []).map(session => ({ ...session, accountId: result.value.account.id, source: result.value.account.source ?? 'wechat' })) : []);
      list.sort((a, b) => (b.last_time ?? 0) - (a.last_time ?? 0));
      const failed = results.flatMap((result, index) => result.status === 'rejected' ? [accounts[index].source === 'whatsapp' ? 'WhatsApp' : t("微信")] : []);
      setSettings(stored);
      setSessions(list.slice(0, 200));
      sessionOffsets.current = new Map(results.flatMap(result => result.status === 'fulfilled' && result.value.sessions.length === 50 ? [[result.value.account.id, { account: result.value.account, offset: 50 }] as const] : []));
      setHasMoreSessions(sessionOffsets.current.size > 0);
      setSelectedSession(current => list.find(item => item.accountId === current?.accountId && item.username === current?.username)
        // A refreshed first page does not prove a previously selected chat was deleted.
        ?? (current && accounts.some(account => account.id === current.accountId) ? current : list[0] ?? null));
      if (failed.length) setError(tf("{0}会话读取失败，请刷新重试。", [...new Set(failed)].join('、')));
      setEventRefresh(value => value + 1);

    } catch (reason) { if (generation === loadGeneration.current) setError(String(reason)); }
    finally { if (generation === loadGeneration.current) setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  useEffect(() => { if (view === 'summary' && visible) void invoke<SummaryRecord[]>('summary_history_list').then(setSummaries).catch(() => setSummaries([])); }, [view, visible]);
  const loadMoreSessions = async (source: string) => {
    if (moreLock.current) return;
    moreLock.current = true; setLoading(true); setMoreSessionsError(null); setLoadingSource(source);
    const generation = loadGeneration.current;
    try {
      // At most two independent source reads in flight, bounded to 50 each.
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
      setSessions(current => {
        const merged = [...new Map([...current, ...added].map(row => [sessionKey(row.accountId, row.username), row])).values()];
        const other = merged.filter(row => row.source !== source).slice(-150);
        const own = merged.filter(row => row.source === source).slice(-(200 - other.length));
        const keep = new Set([...other, ...own].map(row => sessionKey(row.accountId, row.username)));
        return merged.filter(row => keep.has(sessionKey(row.accountId, row.username)));
      });
      setHasMoreSessions(sessionOffsets.current.size > 0);
    } catch (reason) { if (generation === loadGeneration.current) setMoreSessionsError({ source, message: String(reason) }); }
    finally { moreLock.current = false; if (generation === loadGeneration.current) { setLoading(false); setLoadingSource(null); } }
  };
  const wasVisible = useRef(visible);
  useEffect(() => {
    if (visible && !wasVisible.current) void load();
    wasVisible.current = visible;
  }, [visible, load]);

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

  const reloadSummaries = useCallback(async () => {
    try { setSummaries(await invoke<SummaryRecord[]>('summary_history_list')); } catch { /* keep the last list */ }
  }, []);

  const scopedSummaries = useMemo(() => selectedSession
    ? summaries.filter(record => record.account_id === accountId && record.sessions.some(item => item.username === selectedSession.username))
    : summaries, [summaries, selectedSession, accountId]);

  const uploadState = useMemo(() => {
    const pending = new Set<string>(); const local = new Set<string>();
    Object.entries(uploadStates).forEach(([key, item]) => {
      if (item.state === 'pending') pending.add(key);
      else if (item.state === 'local') local.add(key);
    });
    return { pending, local };
  }, [uploadStates]);
  const pendingCount = uploadState.pending.size;
  const localCount = uploadState.local.size;
  const activeAccountId = settings?.active_account_id ?? '';
  const selectedChats = new Set(settings?.chat?.allowed_usernames?.[activeAccountId] ?? []);
  const selectedPending = Object.values(uploadStates).filter(item => item.state === 'pending' && selectedChats.has(item.username));
  const selectedPendingMessages = selectedPending.reduce((sum, item) => sum + item.new_messages, 0);
  const activeUploads = useMemo(() => (settings?.upload_queue ?? []).filter(job => job.status != null && ['queued', 'running', 'processing', 'uploading'].includes(job.status)), [settings]);

  useEffect(() => { setUploadStates({}); }, [settings?.devify?.base_url, settings?.devify?.active_space_id, activeAccountId]);

  // A local badge scan can outlast its refresh interval on a large history. Keep
  // one in flight and stop scheduling while this retained pane is hidden.
  const uploadStateReading = useRef(false);
  const uploadStateScope = JSON.stringify([visible, settings?.devify?.base_url, settings?.devify?.active_space_id, activeAccountId, sessions.map(item => sessionKey(item.accountId, item.username))]);
  const currentUploadStateScope = useRef(uploadStateScope);
  currentUploadStateScope.current = uploadStateScope;
  const refreshUploadState = useCallback(async () => {
    if (!visible || !isTauri() || !activeAccountId || !sessions.length) return;
    if (uploadStateReading.current) return;
    uploadStateReading.current = true;
    const scope = uploadStateScope;
    try {
      const items = await invoke<UploadState[]>('wechat_upload_state', { accountId: activeAccountId, usernames: sessions.filter(session => session.accountId === activeAccountId).map(session => session.username) });
      if (currentUploadStateScope.current === scope) setUploadStates(Object.fromEntries(items.map(item => [sessionKey(activeAccountId, item.username), item])));
    } catch { /* keep the last known badges rather than claiming everything is synced */ }
    finally { uploadStateReading.current = false; }
  }, [visible, activeAccountId, sessions, uploadStateScope]);
  useEffect(() => {
    if (!visible) return;
    void refreshUploadState();
    // Upload badges are rebuilt from the local database, so refresh them on a fixed short
    // cadence: tying this to the (now 30-minute) backup interval would leave status stale.
    const timer = window.setInterval(() => void refreshUploadState(), activeUploads.length || syncNoticePending ? 5000 : 60000);
    return () => window.clearInterval(timer);
  }, [visible, refreshUploadState, activeUploads.length, syncNoticePending]);

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
    // In the raw tab the header search targets message content, so the conversation list
    // must not collapse to sessions whose names happen to contain the keyword.
    const keyword = view === 'raw' ? '' : query.trim().toLowerCase();
    return sessions.filter(session => (!keyword || `${session.display} ${session.username}`.toLowerCase().includes(keyword)) && (filter === 'all' || (filter === 'pending' ? uploadState.pending.has(sessionKey(session.accountId, session.username)) : uploadState.local.has(sessionKey(session.accountId, session.username)))));
  }, [sessions, query, filter, uploadState, view]);
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
  // Sessions arrive sorted by last activity, so a source first shows up at the moment it
  // moved to the top — that order carries the groups. One visible source needs no header.
  const sourceGroups = useMemo(() => {
    const order: string[] = [];
    const bySource = new Map<string, OwnedSession[]>();
    for (const session of sessions) {
      const source = session.source || 'wechat';
      if (!bySource.has(source)) { bySource.set(source, []); order.push(source); }
    }
    for (const session of visibleSessions) {
      const source = session.source || 'wechat';
      if (!bySource.has(source)) { bySource.set(source, []); order.push(source); }
      bySource.get(source)!.push(session);
    }
    return order.map(source => ({ source, sessions: bySource.get(source)! }));
  }, [sessions, visibleSessions]);
  const multiSource = sourceGroups.length > 1;
  const scopedEvents = eventsScope === JSON.stringify([settings?.devify?.base_url, settings?.devify?.active_space_id, accountId, selectedSession?.username]) ? events : [];
  const visibleEvents = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return scopedEvents.filter(event => (!keyword || `${event.title} ${event.content ?? ''}`.toLowerCase().includes(keyword)) && (filter !== 'pending' || event.status !== 'done'));
  }, [scopedEvents, query, filter]);
  const eventGroups = useMemo(() => {
    const groups = new Map<string, EventItem[]>();
    [...visibleEvents]
      .sort((a, b) => (b.last_activity_at ?? '').localeCompare(a.last_activity_at ?? ''))
      .forEach(event => {
        const day = (event.last_activity_at || event.updated_at || '').slice(0, 10) || 'unknown';
        const list = groups.get(day) ?? [];
        list.push(event);
        groups.set(day, list);
      });
    return Array.from(groups.entries());
  }, [visibleEvents]);

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

  // These actions live in the page header so they stay reachable from every tab; the raw
  // pane still owns the messages and the work drawer, so the request is a counter it watches.
  const openWorkPanel = () => {
    setView('raw');
    setWorkRequest(value => value + 1);
  };
  if (!isTauri()) return null;

  const conversation = selectedSession?.display ?? t("全部会话");
  const sessionMeta = (session: OwnedSession) => {
    const state = uploadStates[sessionKey(session.accountId, session.username)];
    if (state?.state === 'pending') return { text: state.new_messages ? tf("本机新增待上传 · {0} 条", state.new_messages) : t("本机新增待上传"), tone: 'pending' };
    if (state?.state === 'local') return { text: t("仅本机 · 未纳入上传"), tone: 'local' };
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
        <p>{loading && !sessions.length ? t("正在读取会话…") : tf("{0} 个会话 · {1} 个有新增待上传", sessions.length, pendingCount)}</p>
      </header>
      <input className="chat-workbench-search" value={query} onChange={event => setQuery(event.target.value)} placeholder={t("搜索会话与聊天记录")}/>
      <div className="chat-workbench-filters">
        <button type="button" className={filter === 'all' ? 'active' : ''} onClick={() => setFilter('all')}>{t("全部")} <small>{sessions.length}</small></button>
        <button type="button" className={filter === 'pending' ? 'active' : ''} onClick={() => setFilter('pending')}>{t("待传")} <small>{pendingCount}</small></button>
        <button type="button" className={filter === 'local' ? 'active' : ''} onClick={() => setFilter('local')}>{t("只在本机")} <small>{localCount}</small></button>
      </div>
      <div className="chat-workbench-session-label"><span>{t("会话 ·")} {visibleSessions.length}</span><span className="chat-workbench-list-tools">{t("按最后更新")} <button type="button" className="chat-workbench-refresh" onClick={() => void load()} aria-label={t("刷新会话")} title={t("刷新会话")}>↻</button></span></div>
      <div className="chat-workbench-sessions" aria-busy={loading} onScroll={event => {
        const list = event.currentTarget;
        if (!visible || !hasMoreSessions || loading) return;
        const viewport = list.getBoundingClientRect();
        for (const marker of list.querySelectorAll<HTMLElement>('[data-more-source]')) {
          const source = marker.dataset.moreSource!;
          const bounds = marker.getBoundingClientRect();
          if (moreSessionsError?.source !== source && bounds.top <= viewport.bottom + 160 && bounds.bottom >= viewport.top) { void loadMoreSessions(source); break; }
        }
      }}>
        {loading && !sessions.length && <LoadingSkeleton label={t("正在加载会话")} />}
        {error && <p className="chat-workbench-load-error" role="alert">{t(error)} <button type="button" onClick={() => void load()}>{t("重试")}</button></p>}
        {sourceGroups.map(group => {
          const collapsed = multiSource && Boolean(collapsedSources[group.source]);
          const label = group.source === 'whatsapp' ? 'WhatsApp' : t("微信");
          const hasMore = [...sessionOffsets.current.values()].some(entry => (entry.account.source ?? 'wechat') === group.source);
          const readError = moreSessionsError?.source === group.source ? moreSessionsError.message : '';
          return <div className="chat-workbench-source-section" key={group.source}>
            {multiSource && <button type="button" className="chat-workbench-source-group" aria-expanded={!collapsed} onClick={() => setCollapsedSources(current => ({ ...current, [group.source]: !current[group.source] }))}>
              <img src={group.source === 'whatsapp' ? whatsappIcon : wechatIcon} alt=""/>
              <span>{t(label)}</span>
              <small>{group.sessions.length}</small>
              <i aria-hidden="true">{collapsed ? '▸' : '▾'}</i>
            </button>}
            {!collapsed && groupSessions(group.sessions).map(item => <Fragment key={item.key}>
              {item.name && <div className="chat-workbench-community-label">{item.name} {t("· 社区")}</div>}
              {item.sessions.map(session => {
                const meta = sessionMeta(session);
                return <button type="button" key={sessionKey(session.accountId, session.username)} className={`chat-workbench-session${selectedSession?.accountId === session.accountId && selectedSession?.username === session.username ? ' selected' : ''}`} onClick={() => { setRawFocus(null); setSelectedSession(session); }}>
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
            {!collapsed && hasMore && <div className="chat-workbench-source-more" data-more-source={group.source}>
              {loadingSource === group.source && <p className="chat-workbench-empty" role="status">{t('正在读取会话…')}</p>}
              {readError && <p className="chat-workbench-load-error" role="alert">{t(readError)} <button type="button" disabled={loading} onClick={() => void loadMoreSessions(group.source)}>{t('重试')}</button></p>}
              {!loading && !readError && (query.trim() || filter !== 'all') && <button type="button" className="connector-quiet" onClick={() => void loadMoreSessions(group.source)}>{t('加载更多会话')}</button>}
            </div>}
          </div>;
        })}
        {!loading && !visibleSessions.length && <p className="chat-workbench-empty">{t("没有符合条件的会话。")}</p>}
      </div>
      <SidebarResizeHandle/>
    </aside>

    <main className="chat-workbench-main">
      <header className="chat-workbench-head">
        <div className="chat-workbench-title">
          {selectedSession && <span className="chat-workbench-title-avatar" aria-hidden="true"><LocalChatAvatar key={sessionKey(selectedSession.accountId, selectedSession.username)} accountId={selectedSession.accountId} username={selectedSession.username} label={selectedSession.display} /></span>}
          <div className="chat-workbench-title-text"><h2>{conversation}</h2>{selectedSession && <p>{sourceLabel}{selectedSession.is_group ? t("群聊") : t("单聊")}{selectedSession.is_group && selectedSession.member_count != null ? tf(" · {0} 人", selectedSession.member_count) : ''}</p>}</div>
        </div>
        <div className="chat-workbench-head-right">
          <div className="chat-workbench-tabs" role="tablist" aria-label={t("聊天视图")}>
            <button type="button" role="tab" aria-selected={view === 'raw'} className={view === 'raw' ? 'active' : ''} onClick={() => setView('raw')}>{t("原始记录")}</button>
            <button type="button" role="tab" aria-selected={view === 'events'} className={view === 'events' ? 'active' : ''} onClick={() => setView('events')}>{t("事件")} <small>{scopedEvents.length}</small></button>
            {internalAccount && <button type="button" role="tab" aria-selected={view === 'summary'} className={view === 'summary' ? 'active' : ''} onClick={() => setView('summary')}>
              {t("聊天总结")}
              {summaryBusy
                ? <i className="chat-workbench-tab-spinner" aria-label={t("正在生成")}/>
                : scopedSummaries.length > 0 && <small>{scopedSummaries.length}</small>}
            </button>}
          </div>
          <div className="chat-workbench-head-tools"><input className="chat-workbench-head-search" aria-label={view === 'raw' ? t("在此会话中搜索") : view === 'summary' ? t("搜索聊天总结") : t("搜索事件")} value={query} onChange={event => setQuery(event.target.value)} placeholder={view === 'raw' ? t("在此会话中搜索") : view === 'summary' ? t("搜索聊天总结") : t("搜索事件")}/>
          {chatState.active && <div className="chat-workbench-head-actions">
            {canOrganize && chatState.selected > 0 && <button type="button" className="selected" onClick={openWorkPanel}>{t("已选")} {chatState.selected} {t("条")}</button>}
            <button type="button" onClick={exportNotSupported}>{t("导出")}</button>
          </div>}</div>
        </div>
      </header>

      <div className={`chat-workbench-pane${view === 'events' ? '' : ' is-hidden'}`}>
        <div className="chat-workbench-subline">
          <span>{eventsLoading ? t("正在同步事件…") : tf("{0} 条事件", scopedEvents.length)} · {sessions.length} {t("个会话 · 云端整理，可人工修改")}</span>
        </div>
        <div className="chat-workbench-feed">
          {eventsLoading && <LoadingSkeleton label={t("正在加载事件")} kind="detail" />}
          {(error || eventsError) && <div className="chat-workbench-error" role="alert"><span>{scopedEvents.length ? t("事件刷新失败，显示的是上次读取结果。") : t("暂时无法读取事件，请检查网络或稍后重试。")}</span> <button type="button" onClick={() => { if (error) void load(); else setEventRefresh(value => value + 1); }}>{t("重试")}</button></div>}
          {eventViewState(eventsLoading, error || eventsError, visibleEvents.length) === 'empty' && <div className="chat-workbench-empty-panel">
            <h2>{t("当前会话暂无事件")}</h2>
            <p>{t("上传当前会话并等待服务端分析后，相关事件会出现在这里。")}</p>
            <button type="button" onClick={onSchedule}>{t("设置备份周期")}</button>
          </div>}
          {eventGroups.map(([day, list]) => <section className="chat-event-day" key={day}>
            <div className="chat-event-divider"><span/>{dayLabel(list[0]?.last_activity_at || day)}<span/></div>
            {list.map(event => {
              const done = event.status === 'done';
              return <div className="chat-event" key={event.id}>
                <div className="chat-event-gutter">
                  <span className={`chat-event-tag${done ? ' done' : ''}`}>{statusLabel(event.status)}</span>
                </div>
                <button type="button" className="chat-event-card" onClick={() => setEvidence({ kind: 'event', id: event.id, title: event.title })}>
                  <span className="chat-event-head">
                    <StateDot done={done}/>
                    <strong>{event.title}</strong>
                  </span>
                  {event.content && <span className="chat-event-content">{event.content}</span>}
                  <span className="chat-event-foot">
                    <span>{dateLabel(event.last_activity_at || event.updated_at)}</span>
                    <span className="chat-event-link">{t("查看完整事件 →")}</span>
                  </span>
                </button>
              </div>;
            })}
          </section>)}
        </div>
      </div>

      {!rawMounted && view === 'raw' && <div className="chat-workbench-empty" role="status">{t("选择一个会话开始查看")}</div>}
      {rawMounted && <div className={`chat-workbench-pane chat-workbench-raw-content chat-workbench-embedded${view === 'raw' ? '' : ' is-hidden'}`}>
        {accountId === activeAccountId && selectedPending.length > 0 && <div className="chat-workbench-banner">
          <span><i/>{t("已选择的聊天中有")} {selectedPending.length} {t("个会话")}{selectedPendingMessages ? tf("、{0} 条", selectedPendingMessages) : ''}{t("新增记录待上传，")}{settings?.sync?.enabled && settings?.backup?.mode !== 'manual' ? t("将在满足条件时自动备份。") : t("自动备份未开启，可以在备份周期中设置。")}</span>
          <div>
            <button type="button" disabled={syncing} onClick={onOpenTasks}>{t("查看任务")}</button>
            <button type="button" className="primary" onClick={onSchedule}>{t("备份周期")}</button>
          </div>
        </div>}
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
          sessionRevision={sessionRevision}
          openSummary={openSummaryRequest}
          onSummaryRequestHandled={() => setOpenSummaryRequest(false)}
          focusConversation={rawFocus?.conversationId ?? selectedSession?.username}
          focusMessageId={rawFocus?.messageId}
          searchKeyword={view === 'raw' ? query : ''}
          onClearSearch={() => setQuery('')}
          workRequest={workRequest}
          onChatState={setChatState}
          onOpenData={onOpenData}
          onOpenWhitelist={onOpenWhitelist}
        />}
      </div>}

      <div className={`chat-workbench-pane${view === 'summary' ? '' : ' is-hidden'}`}>
        <ChatSummary
          accountId={accountId}
          sessions={sessions.filter(session => session.accountId === accountId)}
          session={selectedSession}
          summaries={summaries}
          query={query}
          onReload={reloadSummaries}
          onOpenRaw={openRaw}
          onBusyChange={setSummaryBusy}
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
