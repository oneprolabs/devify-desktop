import { LoadingSkeleton } from './LoadingSkeleton';
import { eventViewState, syncErrorMessage } from './chatPresentation';
import { SidebarResizeHandle } from './SidebarResizeHandle';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { desktopInvoke as invoke } from './desktopInvoke';
import { EvidenceDrawer, type EvidenceTarget } from './EvidenceDrawer';
import WeChatLocal from './WeChatLocal';
import { ChatSummary } from './ChatSummary';
import type { SummaryRecord } from './summary';
import { exportNotSupported } from './toast';
import { WhitelistSettings } from './Settings';
import wechatIcon from './assets/connectors/wechat.svg';
import { syncFeedback, type SyncBatchStatus } from './syncFeedback';
import { requestFeedback } from './desktopInvoke';

type Session = { username: string; display: string; is_group: boolean; last_time: number | null; preview?: string | null; unread?: number | null; member_count?: number | null };
type Settings = { sync?: { enabled?: boolean }; backup?: { mode?: string }; active_account_id?: string; chat?: { sync_interval_seconds?: number; allowed_usernames?: Record<string, string[]> }; devify?: { active_space_id?: string | null; base_url?: string }; upload_queue?: Array<{ status?: string }> };
type SyncResult = { uploaded: number; message_count: number; batch_ids?: number[]; error?: string | null };
type Scan = { accounts?: Array<{ id: string; modified_at?: number | null }> };
type EventItem = { id: number; title: string; content?: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };
type Page<T> = { list?: T[]; pagination?: { total?: number; next?: unknown } };
type CloudConversation = { id: number; platform: string; source_namespace: string; source_conversation_id: string };
type Avatar = { username: string; data_url: string };
// Rust decides this from the newest local message and the newest uploaded position, so a
// failed upload or new local records cannot be hidden behind a queue status.
type UploadState = { username: string; state: 'pending' | 'local' | 'synced' | 'processing' | 'failed' | 'history_failed' | 'unknown'; new_messages: number };

type View = 'events' | 'raw' | 'summary';
type Filter = 'all' | 'pending' | 'local';

const dateLabel = (value?: string | number | null) => value ? new Date(typeof value === 'number' ? value * 1000 : value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '时间未知';
const timeLabel = (value?: number | null) => {
  if (!value) return '';
  const date = new Date(value * 1000);
  const now = new Date();
  const sameDay = date.toDateString() === now.toDateString();
  if (sameDay) return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
};
const dayLabel = (value?: string | null) => {
  if (!value) return '时间未知';
  const date = new Date(value);
  const now = new Date();
  const day = date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
  if (date.toDateString() === now.toDateString()) return `${day} · 今天`;
  const yesterday = new Date(now.getTime() - 86400000);
  if (date.toDateString() === yesterday.toDateString()) return `${day} · 昨天`;
  return day;
};
const statusLabel = (value?: string | null) => ({ open: '待我处理', in_progress: '推进中', done: '已完成' } as Record<string, string>)[value || ''] || '未标记';

function StateDot({ done }: { done: boolean }) {
  return <svg className="chat-event-dot" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25" fill={done ? '#1a1a19' : 'transparent'} stroke={done ? '#1a1a19' : '#c4a06a'} strokeWidth={1.5}/>
    {done && <path d="M5 8.3l2.1 2.1L11 6.1" stroke="#fff" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
  </svg>;
}

export function ChatWorkbench({ startFirstBackup, onFirstBackupStarted, onOpenData, onOpenWhitelist, onNewUpload, onOpenTasks, onSchedule, onOpenAccount, onOpenEvent }: { startFirstBackup?: boolean; onFirstBackupStarted?: () => void; onOpenData: () => void; onOpenWhitelist: () => void; onNewUpload: () => void; onOpenTasks: () => void; onSchedule: () => void; onOpenAccount: () => void; onOpenEvent?: (eventId: number) => void }) {
  const [view, setView] = useState<View>('raw');
  const [addingSession, setAddingSession] = useState(false);
  const [sessionRevision, setSessionRevision] = useState(0);
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [eventsScope, setEventsScope] = useState('');
  const [summaries, setSummaries] = useState<SummaryRecord[]>([]);
  const [summaryBusy, setSummaryBusy] = useState(false);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [accountId, setAccountId] = useState<string | null>(null);
  const [avatars, setAvatars] = useState<Record<string, string>>({});
  const [uploadStates, setUploadStates] = useState<Record<string, UploadState>>({});
  const [syncing, setSyncing] = useState(false);
  const [syncNotice, setSyncNotice] = useState('');
  const [syncNoticeError, setSyncNoticeError] = useState(false);
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
  // The raw pane owns the message list, so it reports back whether 整理/导出 can act.
  const [chatState, setChatState] = useState({ active: false, messages: 0, selected: 0, agentReady: false });
  const [workRequest, setWorkRequest] = useState(0);
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);
  // Mount the raw pane right away (hidden) so the first tab switch does not wait for the
  // conversation + message load; the work happens while the event feed is on screen.
  useEffect(() => { const timer = window.setTimeout(() => setRawMounted(true), 400); return () => window.clearTimeout(timer); }, []);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true); setError('');
    try {
      const [stored, scan] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<Scan>('wechat_scan')]);
      const accounts = scan.accounts ?? [];
      const account = accounts.find(item => item.id === stored.active_account_id) ?? [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
      const list = account ? await invoke<Session[]>('wechat_sessions', { accountId: account.id }) : [];
      setSettings(stored); setAccountId(account?.id ?? null); setSessions(list ?? []); setSelectedSession(current => list.find(item => item.username === current?.username) ?? list[0] ?? null);
      if (account && list?.length) {
        // Real WeChat profile pictures; a missing avatar falls back to the first character.
        void invoke<Avatar[]>('wechat_avatars', { accountId: account.id, usernames: list.slice(0, 200).map(session => session.username) })
          .then(items => setAvatars(Object.fromEntries((items ?? []).map(item => [item.username, item.data_url]))))
          .catch(() => setAvatars({}));
      }
      setEventRefresh(value => value + 1);
      try { setSummaries(await invoke<SummaryRecord[]>('summary_history_list')); } catch { setSummaries([]); }
    } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  useEffect(() => {
    let cancelled = false;
    const spaceId = settings?.devify?.active_space_id;
    const username = selectedSession?.username;
    const requestScope = JSON.stringify([settings?.devify?.base_url, spaceId, accountId, username]);
    setEventsError('');
    if (!isTauri() || !spaceId || !username || !accountId) {
      setEventsLoading(false);
      return () => { cancelled = true; };
    }
    setEventsLoading(true);
    void (async () => {
      try {
        let conversation: CloudConversation | undefined;
        for (let page = 1; page <= 100; page += 1) {
          const result = await invoke<Page<CloudConversation>>('devify_conversations', { spaceId, search: username, page, pageSize: 100 });
          if (cancelled) return;
          if (!Array.isArray(result.list) || result.pagination?.next === undefined) throw new Error('无法完整读取云端会话，请重试。');
          const matches = result.list.filter(item => item.platform === 'wechat' && item.source_namespace === accountId && item.source_conversation_id === username);
          if (matches.length > 1) throw new Error('云端存在多个相同来源的会话，无法安全筛选事件。');
          conversation = matches[0];
          if (conversation || !result.pagination?.next) break;
          if (page === 100) throw new Error('云端会话过多，无法完整筛选事件。');
        }
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
  }, [settings?.devify?.base_url, settings?.devify?.active_space_id, accountId, selectedSession?.username, eventRefresh]);

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
    ? summaries.filter(record => record.sessions.some(item => item.username === selectedSession.username))
    : summaries, [summaries, selectedSession]);

  const uploadState = useMemo(() => {
    const pending = new Set<string>(); const local = new Set<string>();
    Object.values(uploadStates).forEach(item => {
      if (item.state === 'pending') pending.add(item.username);
      else if (item.state === 'local') local.add(item.username);
    });
    return { pending, local };
  }, [uploadStates]);
  const pendingCount = uploadState.pending.size;
  const localCount = uploadState.local.size;
  const selectedChats = new Set(settings?.chat?.allowed_usernames?.[accountId ?? ''] ?? []);
  const selectedPending = Object.values(uploadStates).filter(item => item.state === 'pending' && selectedChats.has(item.username));
  const selectedPendingMessages = selectedPending.reduce((sum, item) => sum + item.new_messages, 0);
  const activeUploads = useMemo(() => (settings?.upload_queue ?? []).filter(job => job.status != null && ['queued', 'running', 'processing', 'uploading'].includes(job.status)), [settings]);

  // Ask Rust again on a timer: new WeChat records land locally while the app is open, and a
  // finished batch only clears the badge once the server reports it is done.
  const refreshUploadState = useCallback(async () => {
    if (!isTauri() || !accountId || !sessions.length) { setUploadStates({}); return; }
    try {
      const items = await invoke<UploadState[]>('wechat_upload_state', { accountId, usernames: sessions.map(session => session.username) });
      setUploadStates(Object.fromEntries(items.map(item => [item.username, item])));
    } catch { /* keep the last known badges rather than claiming everything is synced */ }
  }, [accountId, sessions]);
  useEffect(() => {
    void refreshUploadState();
    // Upload badges are rebuilt from the local database, so refresh them on a fixed short
    // cadence: tying this to the (now 30-minute) backup interval would leave status stale.
    const timer = window.setInterval(() => void refreshUploadState(), activeUploads.length || syncNoticePending ? 5000 : 60000);
    return () => window.clearInterval(timer);
  }, [refreshUploadState, activeUploads.length, syncNoticePending]);

  const syncNow = async () => {
    if (syncing) return;
    setSyncBatchIds([]); setSyncing(true); setSyncNotice(''); setSyncNoticeError(false); setSyncNoticePending(false); setSyncToastVisible(true);
    try {
      const result = await invoke<SyncResult>('devify_sync_now');
      setSyncNoticeError(Boolean(result.error));
      const ids = result.batch_ids ?? [];
      setSyncBatchIds(result.error ? [] : ids);
      setSyncNoticePending(!result.error && ids.length > 0);
      setSyncNotice(result.error
        ? `已提交 ${result.message_count} 条记录；部分会话未完成，请查看任务。`
        : result.message_count
          ? ids.length ? `已提交 ${result.message_count} 条记录，等待服务端处理。` : `已提交 ${result.message_count} 条记录，请在任务中查看状态。`
          : '没有需要提交的新记录。');
      await refreshUploadState();
    } catch (reason) { setSyncNoticeError(!String(reason).includes('该账号正在上传')); setSyncNotice(syncErrorMessage(reason)); }
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
          setSyncNotice('服务端已完成处理。');
          void refreshUploadState();
        } else {
          setSyncNoticeError(true);
          const failed = batches.find(batch => ['failed', 'cancelled'].includes(batch.status));
          setSyncNotice(status === 'unknown' ? '暂时无法确认服务端处理状态，请查看任务。' : failed?.error_message ? `服务端处理未完成：${failed.error_message}` : '服务端处理未完成，请查看任务。');
        }
      } catch {
        if (!active) return;
        setSyncBatchIds([]); setSyncNoticePending(false); setSyncNoticeError(true);
        setSyncNotice('暂时无法确认服务端处理状态，请查看任务。');
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
    return sessions.filter(session => (!keyword || `${session.display} ${session.username}`.toLowerCase().includes(keyword)) && (filter === 'all' || (filter === 'pending' ? uploadState.pending.has(session.username) : uploadState.local.has(session.username))));
  }, [sessions, query, filter, uploadState, view]);
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
    const next = sessions.find(item => item.username === username);
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

  const conversation = selectedSession?.display ?? '全部会话';
  const sessionMeta = (session: Session) => {
    const state = uploadStates[session.username];
    if (state?.state === 'pending') return { text: state.new_messages ? `本机新增待上传 · ${state.new_messages} 条` : '本机新增待上传', tone: 'pending' };
    if (state?.state === 'local') return { text: '仅本机 · 未纳入上传', tone: 'local' };
    if (state?.state === 'processing') return { text: '已提交 · 等待服务端处理', tone: 'pending' };
    if (state?.state === 'failed') return { text: '上传失败 · 请检查任务', tone: 'pending' };
    if (state?.state === 'history_failed') return { text: '历史任务未完成 · 请检查任务', tone: 'pending' };
    if (state?.state === 'synced') return { text: '已同步 · 最近确认', tone: 'synced' };
    return { text: '同步状态待确认', tone: 'local' };
  };

  return <div className="chat-workbench">
    <aside className="chat-workbench-sidebar">
      <header className="chat-workbench-sidebar-head">
        <h1>聊天</h1>
        <div className="chat-workbench-header-actions"><button type="button" className="chat-workbench-add" onClick={() => setAddingSession(true)} aria-label="添加会话" title="添加会话"><span aria-hidden="true">＋</span></button></div>
        <p>{loading && !sessions.length ? '正在读取会话…' : `${sessions.length} 个会话 · ${pendingCount} 个有新增待上传`}</p>
      </header>
      <input className="chat-workbench-search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索会话与聊天记录"/>
      <div className="chat-workbench-filters">
        <button type="button" className={filter === 'all' ? 'active' : ''} onClick={() => setFilter('all')}>全部 <small>{sessions.length}</small></button>
        <button type="button" className={filter === 'pending' ? 'active' : ''} onClick={() => setFilter('pending')}>待上传 <small>{pendingCount}</small></button>
        <button type="button" className={filter === 'local' ? 'active' : ''} onClick={() => setFilter('local')}>仅本机 <small>{localCount}</small></button>
      </div>
      <div className="chat-workbench-session-label"><span>会话 · {visibleSessions.length}</span><span className="chat-workbench-list-tools">按最后更新 <button type="button" className="chat-workbench-refresh" onClick={() => void load()} aria-label="刷新会话" title="刷新会话">↻</button></span></div>
      <div className="chat-workbench-sessions">
        {loading && !sessions.length && <LoadingSkeleton label="正在加载会话" />}
        {visibleSessions.map(session => {
          const meta = sessionMeta(session);
          const avatar = avatars[session.username];
          return <button type="button" key={session.username} className={selectedSession?.username === session.username ? 'selected' : ''} onClick={() => setSelectedSession(session)}>
            <span className="chat-workbench-avatar">
              {avatar ? <img src={avatar} alt=""/> : session.display.slice(0, 1)}
              <i className="chat-workbench-avatar-badge" aria-hidden="true"><img src={wechatIcon} alt="" /></i>
            </span>
            <span className="chat-workbench-session-body">
              <span className="chat-workbench-session-line"><strong>{session.display}</strong><time>{timeLabel(session.last_time)}</time></span>
              <small>{session.preview ? (session.preview.length > 34 ? `${session.preview.slice(0, 34)}…` : session.preview) : '暂无消息预览'}</small>
              <em className={meta.tone}>{meta.text}</em>
            </span>
          </button>;
        })}
        {!loading && !visibleSessions.length && <p className="chat-workbench-empty">没有符合条件的会话。</p>}
      </div>
      <SidebarResizeHandle/>
    </aside>

    <main className="chat-workbench-main">
      <header className="chat-workbench-head">
        <div className="chat-workbench-title">
          {selectedSession && <span className="chat-workbench-title-avatar" aria-hidden="true">{avatars[selectedSession.username] ? <img src={avatars[selectedSession.username]} alt=""/> : selectedSession.display.slice(0, 1)}</span>}
          <div className="chat-workbench-title-text"><h2>{conversation}</h2>{selectedSession && <p>微信{selectedSession.is_group ? '群聊' : '单聊'}{selectedSession.is_group && selectedSession.member_count != null ? ` · ${selectedSession.member_count} 人` : ''}</p>}</div>
        </div>
        <div className="chat-workbench-head-right">
          <div className="chat-workbench-tabs" role="tablist" aria-label="聊天视图">
            <button type="button" role="tab" aria-selected={view === 'raw'} className={view === 'raw' ? 'active' : ''} onClick={() => setView('raw')}>原始记录</button>
            <button type="button" role="tab" aria-selected={view === 'events'} className={view === 'events' ? 'active' : ''} onClick={() => setView('events')}>事件 <small>{scopedEvents.length}</small></button>
            <button type="button" role="tab" aria-selected={view === 'summary'} className={view === 'summary' ? 'active' : ''} onClick={() => setView('summary')}>
              聊天总结
              {summaryBusy
                ? <i className="chat-workbench-tab-spinner" aria-label="正在生成"/>
                : scopedSummaries.length > 0 && <small>{scopedSummaries.length}</small>}
            </button>
          </div>
          <div className="chat-workbench-head-tools"><input className="chat-workbench-head-search" aria-label={view === 'raw' ? '在此会话中搜索' : view === 'summary' ? '搜索聊天总结' : '搜索事件'} value={query} onChange={event => setQuery(event.target.value)} placeholder={view === 'raw' ? '在此会话中搜索' : view === 'summary' ? '搜索聊天总结' : '搜索事件'}/>
          {chatState.active && <div className="chat-workbench-head-actions">
            {chatState.selected > 0 && <button type="button" className="selected" onClick={openWorkPanel}>已选 {chatState.selected} 条</button>}
            {chatState.agentReady && <button type="button" disabled={!chatState.messages} onClick={openWorkPanel}>整理工作内容</button>}
            <button type="button" onClick={exportNotSupported}>导出</button>
          </div>}</div>
        </div>
      </header>

      <div className={`chat-workbench-pane${view === 'events' ? '' : ' is-hidden'}`}>
        <div className="chat-workbench-subline">
          <span>{eventsLoading ? '正在同步事件…' : `${scopedEvents.length} 条事件`} · {sessions.length} 个会话 · 云端整理，可人工修改</span>
        </div>
        <div className="chat-workbench-feed">
          {eventsLoading && <LoadingSkeleton label="正在加载事件" kind="detail" />}
          {(error || eventsError) && <div className="chat-workbench-error" role="alert"><span>{scopedEvents.length ? '事件刷新失败，显示的是上次读取结果。' : '暂时无法读取事件，请检查网络或稍后重试。'}</span> <button type="button" onClick={() => { if (error) void load(); else setEventRefresh(value => value + 1); }}>重试</button></div>}
          {eventViewState(eventsLoading, error || eventsError, visibleEvents.length) === 'empty' && <div className="chat-workbench-empty-panel">
            <h2>当前会话暂无事件</h2>
            <p>上传当前会话并等待服务端分析后，相关事件会出现在这里。</p>
            <button type="button" onClick={onNewUpload}>开始第一次上传</button>
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
                    <span className="chat-event-link">查看完整事件 →</span>
                  </span>
                </button>
              </div>;
            })}
          </section>)}
        </div>
      </div>

      {!rawMounted && view === 'raw' && <LoadingSkeleton label="正在加载聊天记录" kind="messages" />}
      {rawMounted && <div className={`chat-workbench-pane chat-workbench-raw-content chat-workbench-embedded${view === 'raw' ? '' : ' is-hidden'}`}>
        {selectedPending.length > 0 && <div className="chat-workbench-banner">
          <span><i/>已选择的聊天中有 {selectedPending.length} 个会话{selectedPendingMessages ? `、${selectedPendingMessages} 条` : ''}新增记录待上传，{settings?.sync?.enabled && settings?.backup?.mode !== 'manual' ? '将在满足条件时自动备份。' : '自动备份未开启，请点击立即上传。'}</span>
          <div>
            <button type="button" disabled={syncing} onClick={() => void syncNow()}>{syncing ? '正在提交…' : '立即上传'}</button>
            <button type="button" className="primary" onClick={onSchedule}>备份周期</button>
          </div>
        </div>}
        {syncToastVisible && (syncing || syncNotice) && <div className={`sync-feedback-toast${syncNoticeError ? ' is-error' : ''}`} role={syncNoticeError ? 'alert' : 'status'}>
          <span className={`sync-feedback-icon${syncing || syncNoticePending ? ' is-loading' : ''}`} aria-hidden="true">{syncing || syncNoticePending ? '' : syncNoticeError ? '!' : '✓'}</span>
          <span className="sync-feedback-text">{syncing ? '正在提交记录…' : syncNotice}</span>
          {!syncing && (syncNoticeError || syncNotice.includes('后台上传')) && <button type="button" onClick={onOpenTasks}>查看任务</button>}
          {syncNoticeError && <button type="button" onClick={() => requestFeedback('upload', 'chat', syncBatchIds.slice(0, 20))}>反馈此问题</button>}
          <button type="button" onClick={() => setSyncToastVisible(false)} aria-label="关闭提示">×</button>
        </div>}
        <WeChatLocal
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
        />
      </div>}

      <div className={`chat-workbench-pane${view === 'summary' ? '' : ' is-hidden'}`}>
        <ChatSummary
          accountId={accountId}
          sessions={sessions}
          session={selectedSession}
          summaries={summaries}
          query={query}
          onReload={reloadSummaries}
          onOpenRaw={openRaw}
          onBusyChange={setSummaryBusy}
        />
      </div>
    </main>

    {addingSession && <div className="chat-add-overlay" role="presentation"><section className="chat-add-dialog" role="dialog" aria-modal="true" aria-label="添加会话"><button type="button" className="chat-add-close" onClick={() => setAddingSession(false)} aria-label="关闭">×</button><WhitelistSettings onSaved={() => { setAddingSession(false); setSessionRevision(value => value + 1); void load(); }}/></section></div>}
    {evidence && <EvidenceDrawer
      spaceId={settings?.devify?.active_space_id ?? null}
      target={evidence}
      onClose={() => setEvidence(null)}
      onNavigate={setEvidence}
      onOpenDetail={target => { if (target.kind === 'event') openEventPage(target.id); }}
    />}
  </div>;
}
