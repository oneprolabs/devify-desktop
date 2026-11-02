import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import WeChatLocal from './WeChatLocal';
import { CloudRecords, type CloudFocus } from './CloudRecords';

type Session = { username: string; display: string; is_group: boolean; last_time: number | null; preview?: string | null; unread?: number | null };
type Settings = { active_account_id?: string; chat?: { sync_interval_seconds?: number }; devify?: { active_space_id?: number | null }; upload_queue?: Array<{ status?: string; scope?: { usernames?: string[] } | null }> };
type Scan = { accounts?: Array<{ id: string; modified_at?: number | null }> };
type EventItem = { id: number; title: string; content?: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };
type EventDetail = EventItem & { note_ids?: number[]; relation_ids?: number[] };
type EventNote = { id: number; title: string; content: string; start_at?: string | null; end_at?: string | null; evidence?: Array<{ id: number; message_id: number; conversation_id: number; sender_name?: string; occurred_at?: string | null }> };
type Page<T> = { list?: T[]; pagination?: { total?: number } };
type Summary = { id: string; title?: string; kind?: string; range_label?: string; content?: string; created_at?: string };

type View = 'events' | 'raw' | 'summary' | 'cloud';
type Filter = 'all' | 'pending' | 'local';

const dateLabel = (value?: string | number | null) => value ? new Date(typeof value === 'number' ? value * 1000 : value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '时间未知';
const statusLabel = (value?: string | null) => ({ open: '待我处理', in_progress: '推进中', done: '已完成' } as Record<string, string>)[value || ''] || '未标记';

export function ChatWorkbench({ onOpenData, onOpenWhitelist, onNewUpload, onSchedule, onOpenAccount }: { onOpenData: () => void; onOpenWhitelist: () => void; onNewUpload: () => void; onSchedule: () => void; onOpenAccount: () => void }) {
  const [view, setView] = useState<View>('events');
  const [filter, setFilter] = useState<Filter>('all');
  const [query, setQuery] = useState('');
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selectedSession, setSelectedSession] = useState<Session | null>(null);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [selectedEvent, setSelectedEvent] = useState<EventDetail | null>(null);
  const [summaries, setSummaries] = useState<Summary[]>([]);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [loading, setLoading] = useState(true);
  const [eventsLoading, setEventsLoading] = useState(false);
  const [error, setError] = useState('');
  const [eventsError, setEventsError] = useState('');
  const [eventNotes, setEventNotes] = useState<EventNote[]>([]);
  const [openSummaryRequest, setOpenSummaryRequest] = useState(false);
  const [rawFocus, setRawFocus] = useState<{ conversationId: string; messageId?: string | number } | null>(null);
  const [cloudFocus, setCloudFocus] = useState<CloudFocus | null>(null);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true); setError('');
    try {
      const [stored, scan] = await Promise.all([invoke<Settings>('app_settings_get'), invoke<Scan>('wechat_scan')]);
      const accounts = scan.accounts ?? [];
      const account = accounts.find(item => item.id === stored.active_account_id) ?? [...accounts].sort((a, b) => (b.modified_at ?? 0) - (a.modified_at ?? 0))[0];
      const list = account ? await invoke<Session[]>('wechat_sessions', { accountId: account.id }) : [];
      setSettings(stored); setSessions(list ?? []); setSelectedSession(current => current && list.some(item => item.username === current.username) ? current : list[0] ?? null);
      if (stored.devify?.active_space_id) {
        setEventsLoading(true);
        try {
          const result = await invoke<Page<EventItem>>('devify_analysis_list', { kind: 'events', spaceId: stored.devify.active_space_id, page: 1 });
          setEvents(result.list ?? []); setEventsError('');
        } catch (reason) { setEvents([]); setEventsError(String(reason)); }
        finally { setEventsLoading(false); }
      }
      try { setSummaries(await invoke<Summary[]>('summary_history_list')); } catch { setSummaries([]); }
    } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const uploadState = useMemo(() => {
    const pending = new Set<string>(); const uploaded = new Set<string>();
    (settings?.upload_queue ?? []).forEach(job => (job.scope?.usernames ?? []).forEach(username => {
      if (job.status === 'completed') uploaded.add(username); else pending.add(username);
    }));
    return { pending, local: new Set(sessions.filter(session => !uploaded.has(session.username)).map(session => session.username)) };
  }, [settings, sessions]);
  const pendingCount = uploadState.pending.size;
  const localCount = uploadState.local.size;
  const activeUploads = (settings?.upload_queue ?? []).filter(job => job.status != null && ['queued', 'running', 'processing', 'uploading'].includes(job.status));
  useEffect(() => { if (!activeUploads.length) return; const timer = window.setInterval(() => { void load(); }, 5000); return () => window.clearInterval(timer); }, [activeUploads.length, load]);
  const visibleSessions = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return sessions.filter(session => (!keyword || `${session.display} ${session.username}`.toLowerCase().includes(keyword)) && (filter === 'all' || (filter === 'pending' ? uploadState.pending.has(session.username) : uploadState.local.has(session.username))));
  }, [sessions, query, filter, uploadState]);
  const visibleEvents = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return events.filter(event => (!keyword || `${event.title} ${event.content ?? ''}`.toLowerCase().includes(keyword)) && (filter !== 'pending' || event.status !== 'done'));
  }, [events, query, filter]);
  const selected = selectedEvent ?? (visibleEvents[0] ?? null);

  const openEvent = async (event: EventItem) => {
    setSelectedEvent(event); setEventNotes([]);
    if (!settings?.devify?.active_space_id) return;
    try {
      const spaceId = settings.devify.active_space_id;
      setSelectedEvent(await invoke<EventDetail>('devify_analysis_detail', { kind: 'events', spaceId, itemId: event.id }));
      try { setEventNotes(await invoke<EventNote[]>('devify_event_notes', { spaceId, eventId: event.id })); } catch { setEventNotes([]); }
    } catch { /* list content remains visible */ }
  };

  if (!isTauri()) return null;
  if (view === 'raw') return <div className="chat-workbench-raw"><div className="chat-workbench-raw-bar"><button type="button" onClick={() => setView('events')}>← 返回聊天工作台</button><span>原始记录</span></div><div className="chat-workbench-raw-content"><WeChatLocal openSummary={openSummaryRequest} onSummaryRequestHandled={() => setOpenSummaryRequest(false)} focusConversation={rawFocus?.conversationId} focusMessageId={rawFocus?.messageId} onOpenData={onOpenData} onOpenWhitelist={onOpenWhitelist} /></div></div>;

  return <div className={`chat-workbench${view === 'cloud' ? ' chat-workbench-cloud-mode' : ''}`}>
    {view !== 'cloud' && <aside className="chat-workbench-sidebar">
      <header className="chat-workbench-sidebar-head"><div><h1>聊天</h1><p>{sessions.length} 个会话 · {pendingCount} 个有本机新增未上传</p></div><button type="button" className="chat-workbench-refresh" onClick={() => void load()} aria-label="刷新">↻</button></header>
      <input className="chat-workbench-search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索会话与聊天记录" />
      <div className="chat-workbench-filters"><button type="button" className={filter === 'all' ? 'active' : ''} onClick={() => setFilter('all')}>全部 <small>{sessions.length}</small></button><button type="button" className={filter === 'pending' ? 'active' : ''} onClick={() => setFilter('pending')}>待上传 <small>{pendingCount}</small></button><button type="button" className={filter === 'local' ? 'active' : ''} onClick={() => setFilter('local')}>仅本机 <small>{localCount}</small></button></div>
      <div className="chat-workbench-session-label">会话 · {visibleSessions.length} <span>· 按最后更新</span></div>
      <div className="chat-workbench-sessions">{visibleSessions.map(session => <button type="button" key={session.username} className={selectedSession?.username === session.username ? 'selected' : ''} onClick={() => setSelectedSession(session)}><span className="chat-workbench-avatar">{session.display.slice(0, 1)}</span><span className="chat-workbench-session-body"><strong>{session.display}</strong><small>{session.preview || '暂无消息预览'}</small><em>微信 · {dateLabel(session.last_time)}</em></span></button>)}{!visibleSessions.length && <p className="chat-workbench-empty">没有符合条件的会话。</p>}</div>
    </aside>}
    <main className="chat-workbench-main">
      <header className="chat-workbench-head"><div><span className="chat-workbench-source">{view === 'cloud' ? `Devify 空间 · ${settings?.devify?.active_space_id ?? '未选择'}` : `微信 · ${selectedSession?.display || '全部会话'}`}</span><div className="chat-workbench-tabs"><button type="button" className={view === 'events' ? 'active' : ''} onClick={() => setView('events')}>事件 <small>{events.length}</small></button><button type="button" onClick={() => setView('raw')}>原始记录</button><button type="button" className={view === 'cloud' ? 'active' : ''} onClick={() => { setCloudFocus(null); setView('cloud'); }}>云端记录 <small>{view === 'cloud' ? '' : '服务端'}</small></button><button type="button" className={view === 'summary' ? 'active' : ''} onClick={() => setView('summary')}>聊天总结 <small>{summaries.length || ''}</small></button></div></div><div className="chat-workbench-head-actions"><button type="button" onClick={onNewUpload}>立即上传</button><button type="button" onClick={onSchedule}>备份周期</button></div></header>
      {view === 'events' && <><div className="chat-workbench-sync"><span>{activeUploads.length ? `正在处理 ${activeUploads.length} 个上传任务 · 完成后自动刷新事件` : '云端事件 · 服务端整理，可人工查看'}</span><span>{eventsLoading ? '同步中…' : eventsError ? '同步失败' : `共 ${events.length} 条`}</span><button type="button" onClick={onNewUpload}>上传并立即分析</button></div><div className="chat-workbench-events-grid"><div className="chat-workbench-event-list">{(error || eventsError) && <p className="chat-workbench-error">{error || eventsError}</p>}{visibleEvents.map(event => <button type="button" className={`chat-workbench-event ${selected?.id === event.id ? 'selected' : ''}`} key={event.id} onClick={() => void openEvent(event)}><span className="chat-workbench-event-date">{dateLabel(event.last_activity_at || event.updated_at)}</span><span className="chat-workbench-event-body"><strong>{event.title}</strong><small className={event.status === 'done' ? 'done' : ''}>{statusLabel(event.status)}</small><p>{event.content || '服务端尚未返回事件摘要。'}</p><em>查看原文依据 · 完整事件 →</em></span></button>)}{!eventsLoading && !visibleEvents.length && <div className="chat-workbench-empty-panel"><h2>还没有整理出事件</h2><p>上传聊天记录并等待服务端分析后，事件会出现在这里。</p><button type="button" onClick={onNewUpload}>开始第一次上传</button></div>}</div><article className="chat-workbench-event-detail">{selected ? <><div className="chat-workbench-event-meta"><span>{statusLabel(selected.status)}</span><time>{dateLabel(selected.last_activity_at || selected.updated_at)}</time></div><h2>{selected.title}</h2><p className="chat-workbench-event-content">{selected.content || '服务端尚未返回事件内容。'}</p><section className="chat-workbench-event-source"><strong>原文依据与进展</strong>{eventNotes.length ? eventNotes.map(note => { const evidence = note.evidence?.[0]; const source = evidence ? <button type="button" className="chat-workbench-source-link" onClick={() => { setCloudFocus({ conversationId: evidence.conversation_id, messageId: evidence.message_id }); setView('cloud'); }}>查看云端原文 →</button> : null; return <div key={note.id}><time>{dateLabel(note.start_at || note.end_at)}</time><span><b>{note.title}</b><small>{note.content}</small>{source}</span></div>; }) : <p>事件关联的原文和进展会在服务端同步后显示。</p>}</section></> : <div className="chat-workbench-empty-panel"><h2>选择一个事件</h2><p>从左侧选择事件查看内容与原文依据。</p></div>}</article></div></>}
      {view === 'summary' && <><div className="chat-workbench-summary-toolbar"><span>本地保存的聊天总结</span><button type="button" onClick={() => { setOpenSummaryRequest(true); setView('raw'); }}>＋ 生成聊天总结</button></div><div className="chat-workbench-summary-list">{summaries.map(summary => <article key={summary.id}><span>{summary.kind || '聊天总结'}</span><h2>{summary.title || summary.range_label || '总结记录'}</h2><p>{summary.content || '点击设置 / 总结记录查看完整内容。'}</p><small>{summary.created_at || ''}</small></article>)}{!summaries.length && <div className="chat-workbench-empty-panel"><h2>还没有聊天总结</h2><p>可以从聊天记录中选择会话，整理成工作内容总结。</p><button type="button" onClick={() => setView('raw')}>打开原始记录</button></div>}</div></>}
      {view === 'cloud' && <CloudRecords
        spaceId={settings?.devify?.active_space_id ?? null}
        focus={cloudFocus}
        onFocusHandled={() => setCloudFocus(null)}
        onOpenSettings={onOpenAccount}
      />}
    </main>
  </div>;
}
