import { SidebarResizeHandle } from './SidebarResizeHandle';
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { EvidenceDrawer, type EvidenceTarget } from './EvidenceDrawer';
import { exportNotSupported } from './toast';

type Kind = 'events' | 'reports';
type EventItem = { id: number; title: string; content: string; status?: string | null; tags?: Array<{ label?: string }>; last_activity_at?: string | null; opened_at?: string | null; updated_at?: string | null };
type RelatedEvent = { id: number; title?: string; status?: string | null; relation_type?: string; type?: string; label?: string; relation_id?: number };
type EventRelation = Omit<RelatedEvent, 'id'> & { id?: number; event_id?: number; related_event_id?: number; source_event_id?: number; target_event_id?: number };
type EventRelationRecord = { id: number; space_id: number; left_event_id: number; right_event_id: number; relation_kind: string; decision: string; reason?: string };
type ReportSection = { id: number; title: string; position: number; items?: Array<{ id: number; title: string; summary: string; label?: string; time_text?: string; contact_text?: string; followup_text?: string; sources?: Array<{ display_text?: string; event_id?: number | null; note_id?: number | null }> }> };
type ReportItem = { id: number; title: string; body?: string; content?: unknown; status: string; period_start?: string | null; period_end?: string | null; generated_at?: string | null; updated_at?: string | null; sections?: ReportSection[] };
type EventNote = { id: number; title: string; content: string; start_at?: string | null; end_at?: string | null; origin?: string | null };
type EventDetail = EventItem & {
  note_ids?: number[] | string;
  relation_ids?: number[] | string;
  related_event_ids?: number[];
  relations?: EventRelation[];
  related_events?: EventRelation[];
};
type Page<T> = { list: T[]; pagination?: { total?: number; page?: number; next?: string | null } };
type AppSettings = { devify?: { active_space_id: number | null } };

const labels: Record<Kind, { title: string; empty: string }> = {
  events: { title: '事件', empty: '当前空间还没有服务端事件。' },
  reports: { title: '报告', empty: '当前空间还没有服务端报告。' },
};
const REPORT_TYPES = [['all', '全部'], ['日报', '日报'], ['周报', '周报'], ['月报', '月报'], ['主题', '主题']];

function date(value?: string | null): string {
  if (!value) return '时间未知';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });
}
function shortDate(value?: string | null): string {
  if (!value) return '';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
function clockDate(value?: string | null): string {
  if (!value) return '—';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
const RELATION_KINDS: Record<string, string> = { same: '同一件事', related: '相关', depends_on: '依赖', separate: '无关' };
function relationKindLabel(kind?: string | null): string {
  return RELATION_KINDS[kind ?? ''] ?? '关联';
}
function statusLabel(value?: string | null): string {
  return ({ open: '进行中', in_progress: '推进中', done: '已完成', queued: '排队中', ready: '已生成', partial: '部分生成', failed: '生成失败' } as Record<string, string>)[value || ''] || value || '未标记';
}
/** Notes arrive as `［会话标签］会话名  #消息区间`; split the tag off and drop the
 *  message-range suffix, which is ingestion bookkeeping rather than a name. */
function noteParts(note: EventNote): { tag: string; conversation: string; lines: string[]; messages: RawMessage[] } {
  const match = /^[［\[]([^］\]]+)[］\]]\s*(.*)$/.exec(note.title ?? '');
  const lines = (note.content ?? '').split('\n').map(line => line.trim()).filter(Boolean);
  const name = ((match ? match[2] : note.title) || `记录 #${note.id}`).replace(/\s*#\d+\s*-\s*\d+\s*$/, '').trim();
  return {
    tag: match ? match[1] : '',
    conversation: name || `记录 #${note.id}`,
    lines,
    messages: lines.map(parseMessage),
  };
}
type RawMessage = { stamp: string; who: string; text: string };
/** One stored line is `[2026-10-29 11:19] 发送者: 正文`; the design shows the
 *  sender and a compact timestamp above the message body. */
function parseMessage(line: string): RawMessage {
  const match = /^\[([^\]]+)\]\s*([^:：]{1,40})[:：]\s*([\s\S]*)$/.exec(line);
  if (!match) return { stamp: '', who: '', text: line };
  return { stamp: compactStamp(match[1]), who: match[2].trim(), text: match[3].trim() };
}
/** Notes arrive in link order; after a re-assign the link moves to the end, so
 *  the timeline is ordered by business time instead. */
function sortNotes(notes: EventNote[]): EventNote[] {
  return [...notes].sort((left, right) => {
    const a = left.start_at ? Date.parse(left.start_at) : Number.NaN;
    const b = right.start_at ? Date.parse(right.start_at) : Number.NaN;
    if (Number.isNaN(a) && Number.isNaN(b)) return left.id - right.id;
    if (Number.isNaN(a)) return 1;
    if (Number.isNaN(b)) return -1;
    return a - b;
  });
}
function compactStamp(value: string): string {
  const match = /(\d{4})-(\d{2})-(\d{2})[ T](\d{2}:\d{2})/.exec(value);
  return match ? `${match[2]}/${match[3]} ${match[4]}` : value;
}
function idList(value?: number[] | string): number[] {
  if (Array.isArray(value)) return value.filter(item => Number.isFinite(item));
  if (typeof value !== 'string') return [];
  try {
    const parsed = JSON.parse(value);
    return Array.isArray(parsed) ? parsed.map(Number).filter(Number.isFinite) : [];
  } catch {
    return [];
  }
}

function DoneDot({ done }: { done: boolean }) {
  return <svg className="analysis-dot" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25" fill={done ? '#1a1a19' : 'transparent'} stroke={done ? '#1a1a19' : '#c4a06a'} strokeWidth={1.5}/>
    {done && <path d="M5 8.3l2.1 2.1L11 6.1" stroke="#fff" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
  </svg>;
}

function genRange(type: string): { start: Date; end: Date; hint: string; title: string } {
  const now = new Date();
  const start = new Date(now);
  if (type === '周报') {
    const offset = (now.getDay() + 6) % 7;
    start.setDate(now.getDate() - offset);
    start.setHours(0, 0, 0, 0);
    const end = new Date(start);
    end.setDate(start.getDate() + 6);
    return { start, end: now, hint: `本周 · ${fmt(start)} – ${fmt(end)}`, title: `周报 · 第 ${isoWeek(now)} 周` };
  }
  if (type === '月报') {
    start.setDate(1);
    start.setHours(0, 0, 0, 0);
    return { start, end: now, hint: `本月 · ${fmt(start)} – ${fmt(now)}`, title: `月报 · ${now.getMonth() + 1} 月` };
  }
  start.setHours(0, 0, 0, 0);
  return { start, end: now, hint: `今天 · ${fmt(now)}`, title: `日报 · ${fmt(now)}` };
}
function fmt(date: Date): string {
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
}
function isoWeek(date: Date): number {
  const target = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const day = target.getUTCDay() || 7;
  target.setUTCDate(target.getUTCDate() + 4 - day);
  const yearStart = new Date(Date.UTC(target.getUTCFullYear(), 0, 1));
  return Math.ceil((((target.getTime() - yearStart.getTime()) / 86400000) + 1) / 7);
}

function reportType(report: ReportItem): string {
  if (report.title.includes('周报')) return '周报';
  if (report.title.includes('月报')) return '月报';
  if (report.title.includes('日报')) return '日报';
  return '报告';
}
function reportBadge(report: ReportItem): string {
  return { 日报: '日', 周报: '周', 月报: '月' }[reportType(report)] ?? '报';
}
function sectionItems(report: ReportItem): string[] {
  return (report.sections ?? []).flatMap(section => (section.items ?? []).map(item => item.title));
}

export function CloudAnalysisPage({ kind, focusId, onOpenEvent }: { kind: Kind; focusId?: number | null; onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [items, setItems] = useState<Array<EventItem | ReportItem>>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [loading, setLoading] = useState(isTauri());
  const [error, setError] = useState('');
  const [total, setTotal] = useState(0);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState('all');
  const [detail, setDetail] = useState<EventDetail | ReportItem | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [eventNotes, setEventNotes] = useState<EventNote[]>([]);
  const [openNotes, setOpenNotes] = useState<Record<number, boolean>>({});
  const [reloadKey, setReloadKey] = useState(0);
  const [creating, setCreating] = useState(false);
  const [genType, setGenType] = useState('日报');
  const [generating, setGenerating] = useState(false);
  const [genError, setGenError] = useState('');
  const [relations, setRelations] = useState<EventRelationRecord[]>([]);
  const [editing, setEditing] = useState(false);
  const [editTitle, setEditTitle] = useState('');
  const [editContent, setEditContent] = useState('');
  const [actionBusy, setActionBusy] = useState<'save' | 'status' | 'link' | 'unlink' | null>(null);
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);
  const [actionError, setActionError] = useState('');
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerQuery, setPickerQuery] = useState('');
  const [removedNotes, setRemovedNotes] = useState<EventNote[]>([]);
  const [addingProgress, setAddingProgress] = useState(false);
  const [progressText, setProgressText] = useState('');
  const pendingSelect = useRef<number | null>(null);
  const copy = labels[kind];
  const isDone = (status?: string | null) => status === 'done';

  useEffect(() => {
    let active = true;
    setItems([]); setSelectedId(null); setDetail(null); setError(''); setQuery(''); setFilter('all');
    setRelations([]); setEditing(false); setPickerOpen(false); setActionError(''); setActionBusy(null); setRemovedNotes([]); setAddingProgress(false); setProgressText('');
    if (!isTauri()) { setLoading(false); return; }
    setLoading(true);
    void invoke<AppSettings>('app_settings_get')
      .then(settings => {
        if (!active) return;
        const id = settings.devify?.active_space_id ?? null;
        setSpaceId(id);
        if (!id) throw new Error('请先在「设置 / 账户」中登录 Devify 并选择空间。');
        return invoke<Page<EventItem> | Page<ReportItem>>('devify_analysis_list', { kind, spaceId: id, page: 1 });
      })
      .then(result => {
        if (!active || !result) return;
        const list = result.list ?? [];
        setItems(list);
        setTotal(result.pagination?.total ?? list.length);
        const pending = pendingSelect.current;
        pendingSelect.current = null;
        setSelectedId(pending && list.some(item => item.id === pending)
          ? pending
          : focusId && (kind === 'events' || list.some(item => item.id === focusId)) ? focusId : list[0]?.id ?? null);
      })
      .catch(reason => { if (active) setError(String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [kind, focusId, reloadKey]);

  const counts = useMemo(() => {
    const open = items.filter(item => !isDone((item as EventItem).status)).length;
    return { open, done: items.length - open };
  }, [items]);

  const filteredItems = useMemo(() => items.filter(item => {
    const text = `${item.title} ${kind === 'events' ? (item as EventItem).content : ''}`.toLowerCase();
    if (query.trim() && !text.includes(query.trim().toLowerCase())) return false;
    if (kind === 'events') {
      if (filter === 'open') return !isDone((item as EventItem).status);
      if (filter === 'done') return isDone((item as EventItem).status);
      return true;
    }
    if (filter !== 'all') return item.title.includes(filter);
    return true;
  }), [items, kind, query, filter]);
  useEffect(() => { if (selectedId != null && kind === 'events' && items.length && !filteredItems.some(item => item.id === selectedId) && !detail && selectedId !== focusId) setSelectedId(filteredItems[0]?.id ?? null); }, [filteredItems, selectedId, kind, items.length, detail, focusId]);
  const selected = useMemo(() => items.find(item => item.id === selectedId) ?? null, [items, selectedId]);

  useEffect(() => {
    if (!isTauri() || !spaceId || selectedId == null) { setDetail(null); setEventNotes([]); return; }
    let active = true;
    setDetailLoading(true);
    setOpenNotes({});
    setEditing(false);
    setActionError('');
    setRemovedNotes([]);
    setAddingProgress(false);
    setProgressText('');
    void invoke<EventDetail | ReportItem>('devify_analysis_detail', { kind, spaceId, itemId: selectedId })
      .then(value => { if (active) setDetail(value); })
      .catch(reason => { if (active) setError(String(reason)); })
      .finally(() => { if (active) setDetailLoading(false); });
    return () => { active = false; };
  }, [kind, spaceId, selectedId]);
  useEffect(() => {
    if (!isTauri() || kind !== 'events' || !spaceId || selectedId == null) { setEventNotes([]); return; }
    let active = true;
    void invoke<EventNote[]>('devify_event_notes', { spaceId, eventId: selectedId })
      .then(notes => { if (active) setEventNotes(sortNotes(notes ?? [])); })
      .catch(() => { if (active) setEventNotes([]); });
    return () => { active = false; };
  }, [kind, spaceId, selectedId]);
  useEffect(() => {
    if (!isTauri() || kind !== 'events' || !spaceId) { setRelations([]); return; }
    let active = true;
    void invoke<EventRelationRecord[]>('devify_event_relations', { spaceId })
      .then(list => { if (active) setRelations(list ?? []); })
      .catch(() => { if (active) setRelations([]); });
    return () => { active = false; };
  }, [kind, spaceId, reloadKey]);

  const report = kind === 'reports' && (detail || selected) ? (detail || selected) as ReportItem : null;
  const event = kind === 'events' && (detail || selected) ? (detail || selected) as EventDetail : null;
  const relatedEvents = useMemo(() => {
    if (!event) return [];
    const byId = new Map<number, RelatedEvent>();
    const eventById = new Map<number, EventItem>(items.filter(item => 'content' in item).map(item => [item.id, item as EventItem]));
    relations.forEach(relation => {
      const otherId = relation.left_event_id === event.id
        ? relation.right_event_id
        : relation.right_event_id === event.id ? relation.left_event_id : null;
      if (!otherId || otherId === event.id) return;
      const known = eventById.get(otherId);
      byId.set(otherId, {
        id: otherId,
        title: known?.title,
        status: known?.status,
        relation_type: relationKindLabel(relation.relation_kind),
        relation_id: relation.id,
      });
    });
    // A relation row the space-wide list has not refreshed yet still belongs here.
    (event.relations ?? event.related_events ?? []).forEach(item => {
      const id = item.event_id ?? item.related_event_id ?? item.target_event_id ?? item.source_event_id ?? item.id;
      if (id && id !== event.id && !byId.has(id)) byId.set(id, { ...item, id });
    });
    // `relation_ids` are EventRelation primary keys, not event ids, so only the
    // explicit related-event id list may add rows here.
    idList(event.related_event_ids).forEach(id => { if (id !== event.id && !byId.has(id)) byId.set(id, { id }); });
    return Array.from(byId.values());
  }, [event, relations, items]);
  const linkCandidates = useMemo(() => {
    if (!event) return [];
    const linked = new Set(relatedEvents.map(relation => relation.id));
    const needle = pickerQuery.trim().toLowerCase();
    return items
      .filter((item): item is EventItem => 'content' in item)
      .filter(item => item.id !== event.id && !linked.has(item.id))
      .filter(item => !needle || item.title.toLowerCase().includes(needle));
  }, [event, items, relatedEvents, pickerQuery]);
  const eventTags = useMemo(() => (event?.tags ?? []).map(tag => tag.label).filter((label): label is string => !!label), [event]);
  const eventPeople = useMemo(() => {
    const people = new Set<string>();
    eventNotes.forEach(note => noteParts(note).messages.forEach(message => { if (message.who) people.add(message.who); }));
    return people.size;
  }, [eventNotes]);
  const timelineRows = useMemo(() => [
    ...eventNotes.map(note => ({ kind: 'note' as const, note })),
    ...relatedEvents.map(relation => ({ kind: 'link' as const, relation })),
  ], [eventNotes, relatedEvents]);
  const allRawOpen = eventNotes.length > 0 && eventNotes.every(note => openNotes[note.id]);
  const toggleAllRaw = () => setOpenNotes(allRawOpen ? {} : Object.fromEntries(eventNotes.map(note => [note.id, true])));
  const reportContent = report?.body || (typeof report?.content === 'string' ? report.content : report?.content ? JSON.stringify(report.content, null, 2) : '服务端尚未返回正文。');
  const applyEventUpdate = (updated: EventItem) => {
    setDetail(current => (current && 'content' in current && (current as EventItem).id === updated.id
      ? { ...(current as EventDetail), ...updated }
      : current));
    setItems(current => current.map(item => (item.id === updated.id ? { ...item, ...updated } : item)));
  };
  const startEdit = () => {
    if (!event) return;
    setEditTitle(event.title ?? '');
    setEditContent(event.content ?? '');
    setActionError('');
    setEditing(true);
  };
  const saveEventEdit = async () => {
    if (!event || !spaceId || actionBusy) return;
    setActionBusy('save');
    setActionError('');
    try {
      const updated = await invoke<EventItem>('devify_event_update', {
        spaceId, eventId: event.id, title: editTitle.trim(), content: editContent,
      });
      if (updated?.id) applyEventUpdate(updated);
      setEditing(false);
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const toggleEventStatus = async () => {
    if (!event || !spaceId || actionBusy) return;
    setActionBusy('status');
    setActionError('');
    try {
      const updated = await invoke<EventItem>('devify_event_update', {
        spaceId, eventId: event.id, status: isDone(event.status) ? 'open' : 'done',
      });
      if (updated?.id) applyEventUpdate(updated);
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const refreshRelations = async () => {
    if (!spaceId) return;
    const list = await invoke<EventRelationRecord[]>('devify_event_relations', { spaceId });
    setRelations(list ?? []);
  };
  const linkEvent = async (targetId: number) => {
    if (!event || !spaceId || actionBusy) return;
    setActionBusy('link');
    setActionError('');
    try {
      await invoke<EventRelationRecord>('devify_event_relation_create', {
        spaceId, leftEventId: event.id, rightEventId: targetId, relationKind: 'related', decision: 'accepted',
      });
      setPickerOpen(false);
      await refreshRelations();
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const removeNote = async (note: EventNote) => {
    if (!spaceId || !event || actionBusy) return;
    setActionBusy('unlink');
    setActionError('');
    try {
      await invoke('devify_event_note_remove', { spaceId, eventId: event.id, noteId: note.id });
      setEventNotes(current => current.filter(item => item.id !== note.id));
      setRemovedNotes(current => [...current, note]);
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const undoRemoveNotes = async () => {
    if (!spaceId || !event || !removedNotes.length || actionBusy) return;
    setActionBusy('unlink');
    setActionError('');
    try {
      const notes = [...removedNotes];
      for (const note of notes) {
        await invoke('devify_event_note_assign', { spaceId, eventId: event.id, noteId: note.id });
      }
      setRemovedNotes([]);
      const restored = await invoke<EventNote[]>('devify_event_notes', { spaceId, eventId: event.id });
      setEventNotes(sortNotes(restored ?? []));
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const addProgress = async () => {
    if (!spaceId || !event || actionBusy) return;
    if (!progressText.trim()) {
      setActionError('请先填写进展内容。');
      return;
    }
    setActionBusy('save');
    setActionError('');
    try {
      await invoke('devify_event_note_create', {
        spaceId,
        eventId: event.id,
        text: progressText,
        startAt: new Date().toISOString(),
      });
      setProgressText('');
      setAddingProgress(false);
      const notes = await invoke<EventNote[]>('devify_event_notes', { spaceId, eventId: event.id });
      setEventNotes(sortNotes(notes ?? []));
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const unlinkEvent = async (relationId: number) => {
    if (!spaceId || actionBusy) return;
    setActionBusy('unlink');
    setActionError('');
    try {
      await invoke('devify_event_relation_delete', { spaceId, relationId });
      await refreshRelations();
    } catch (reason) {
      setActionError(String(reason));
    } finally {
      setActionBusy(null);
    }
  };
  const openRelatedEvent = (eventId: number) => {
    if (kind === 'events') {
      if (!items.some(item => item.id === eventId)) setItems(current => [{ id: eventId, title: `事件 #${eventId}`, content: '' }, ...current]);
      setSelectedId(eventId);
    } else onOpenEvent?.(eventId);
  };
  const generateReport = async () => {
    if (!spaceId || generating) return;
    setGenerating(true);
    setGenError('');
    try {
      const range = genRange(genType);
      const created = await invoke<{ data?: { id?: number } } | { id?: number }>('devify_report_create', {
        spaceId,
        title: range.title,
        periodStart: range.start.toISOString(),
        periodEnd: range.end.toISOString(),
      });
      const id = (created as { data?: { id?: number } }).data?.id ?? (created as { id?: number }).id;
      if (id) pendingSelect.current = id;
      setCreating(false);
      setReloadKey(key => key + 1);
    } catch (reason) {
      setGenError(String(reason));
    } finally {
      setGenerating(false);
    }
  };

  const cycleStatus = () => setFilter(current => kind === 'reports' ? current : current === 'all' ? 'open' : current === 'open' ? 'done' : 'all');
  const filterLabel = kind === 'events'
    ? (filter === 'open' ? '未完成' : filter === 'done' ? '已完成' : '全部')
    : (filter === 'all' ? '全部' : filter);
  const filterPills: Array<[string, string, number]> = kind === 'events'
    ? [['all', '全部', items.length], ['open', '未完成', counts.open], ['done', '已完成', counts.done]]
    : REPORT_TYPES.map(([value, label]) => [value, label, value === 'all' ? items.length : items.filter(item => reportType(item as ReportItem) === value).length]);

  if (!isTauri()) return <div className="analysis-page"><div className="analysis-empty"><h2>请在桌面应用中查看</h2><p>事件和报告由 Devify 服务端生成，桌面端只负责同步并展示。</p></div></div>;
  if (loading) return <div className="analysis-page"><div className="analysis-empty"><h2>正在同步{copy.title}…</h2><p>正在从当前 Devify 空间读取最新结果。</p></div></div>;
  if (error) return <div className="analysis-page"><div className="analysis-empty error" role="alert"><h2>同步失败</h2><p>{error}</p></div></div>;
  if (!items.length) return <div className="analysis-page"><div className="analysis-empty"><h2>{copy.empty}</h2><p>完成聊天采集并等待服务端处理后，结果会出现在这里。</p></div></div>;

  return <div className={`analysis-page${kind === 'events' ? ' event-workspace' : ''}`}>
    <aside className="analysis-side" aria-label={`${copy.title}列表`}>
      <h1>{copy.title}</h1>
      <p className="analysis-side-count">{kind === 'events' ? `${counts.open} 个未完成 · 跨全部会话` : `${items.length} 份报告`}</p>

      <div className="analysis-search">
        <input value={query} spellCheck={false} placeholder={`搜索${copy.title}`} aria-label={`搜索${copy.title}`} onChange={change => setQuery(change.target.value)}/>
      </div>

      {kind === 'reports' && <button type="button" className={`analysis-side-new${creating ? ' on' : ''}`} onClick={() => { setCreating(true); setGenError(''); }}>＋ 生成报告</button>}

      <div className="analysis-side-row">
        <span>{copy.title}</span>
        <button type="button" onClick={cycleStatus}>{filterLabel} ▾</button>
      </div>

      <div className="analysis-pills" role="group" aria-label="筛选">
        {filterPills.map(([value, label, count]) => <button
          key={value}
          type="button"
          aria-pressed={filter === value}
          className={filter === value ? 'on' : ''}
          onClick={() => setFilter(value)}
        >{label}<small>{count}</small></button>)}
      </div>

      <div className="analysis-items">
        {filteredItems.map(item => {
          const status = kind === 'events' ? (item as EventItem).status : (item as ReportItem).status;
          const when = kind === 'events' ? (item as EventItem).last_activity_at : (item as ReportItem).generated_at;
          return <button
            type="button"
            key={item.id}
            className={item.id === selectedId ? 'on' : ''}
            onClick={() => { setCreating(false); setSelectedId(item.id); }}
          >
            {kind === 'reports'
              ? <span className="report-mini-badge">{reportBadge(item as ReportItem)}</span>
              : <span className="event-list-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round"><path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/></svg></span>}
            <span>
              <strong>{item.title || `${copy.title} #${item.id}`}</strong>
              {kind === 'events' && <small className="event-list-preview">{(item as EventItem).content || '暂无事件概述'}</small>}
              <small>{kind === 'reports' ? reportType(item as ReportItem) : statusLabel(status)}{when ? ` · ${shortDate(when)}` : ''}</small>
            </span>
          </button>;
        })}
        {!filteredItems.length && <p className="analysis-list-empty">没有符合筛选条件的{copy.title}</p>}
      </div>
      {kind === 'events' && <SidebarResizeHandle/>}
    </aside>

    <main className="analysis-main">
      <header className="analysis-top">
        <div className="analysis-crumbs"><span className="analysis-crumb-root">{copy.title}</span><span className="analysis-crumb-sep">/</span><span className="analysis-crumb-current">{filterLabel}</span></div>
        <div className="analysis-top-actions">
          {kind === 'reports' && report && <>
            <button type="button" onClick={exportNotSupported}>导出 Markdown</button>
            <button type="button" className="primary" onClick={exportNotSupported}>导出 PDF</button>
          </>}
          {kind === 'events' && event && <>
            <button type="button" disabled={!!actionBusy} onClick={startEdit}>编辑</button>
            <button type="button" disabled={!!actionBusy} onClick={() => { setActionError(''); setPickerQuery(''); setPickerOpen(true); }}>关联事件</button>
            {eventNotes.length > 0 && <button type="button" onClick={toggleAllRaw}>{allRawOpen ? '收起全部原文' : '查看全部原文'}</button>}
            <button
              type="button"
              className={isDone(event.status) ? '' : 'primary'}
              disabled={actionBusy === 'status'}
              onClick={() => void toggleEventStatus()}
            >{actionBusy === 'status' ? '保存中…' : isDone(event.status) ? '重新打开' : '标记完成'}</button>
          </>}
          <button type="button" onClick={() => setReloadKey(key => key + 1)}>刷新</button>
        </div>
      </header>

      <div className="analysis-doc-wrap">
        {kind === 'reports' && creating && <div className="report-create">
          <header>
            <h2>生成报告</h2>
            <p>日报、周报、月报会按时自动生成；这里可以随时按需补生成一份。</p>
          </header>
          <div className="report-create-grid">
            <span className="report-create-label">类型</span>
            <div className="report-create-field">
              <div className="report-seg" role="group" aria-label="报告类型">
                {['日报', '周报', '月报'].map(type => <button
                  key={type}
                  type="button"
                  aria-pressed={genType === type}
                  className={genType === type ? 'on' : ''}
                  onClick={() => setGenType(type)}
                >{type}</button>)}
              </div>
              <small>{genRange(genType).hint}</small>
            </div>
          </div>
          {genError && <p className="analysis-empty error" role="alert">{genError}</p>}
          <button type="button" className="report-create-submit" disabled={generating} onClick={() => void generateReport()}>{generating ? '正在提交…' : '生成'}</button>
          <p className="report-create-note">服务端会按所选区间重新汇总事件；生成完成后会出现在左侧列表，标题为「{genRange(genType).title}」。</p>
        </div>}

        {!(kind === 'reports' && creating) && <>
        {detailLoading && !detail && <p className="analysis-loading">正在读取详情…</p>}

        {event && <article className="analysis-doc">
          {editing ? <div className="analysis-edit">
            <input
              className="analysis-edit-title"
              value={editTitle}
              aria-label="事件标题"
              onChange={change => setEditTitle(change.target.value)}
            />
            <label className="analysis-edit-label" htmlFor="event-edit-summary">摘要</label>
            <textarea
              id="event-edit-summary"
              value={editContent}
              rows={4}
              onChange={change => setEditContent(change.target.value)}
            />
            {actionError && <p className="analysis-action-error" role="alert">{actionError}</p>}
            <div className="analysis-edit-foot">
              <span>保存后以你的版本为准，云端不会覆盖</span>
              <button type="button" disabled={actionBusy === 'save'} onClick={() => { setEditing(false); setActionError(''); }}>取消</button>
              <button type="button" className="primary" disabled={actionBusy === 'save'} onClick={() => void saveEventEdit()}>{actionBusy === 'save' ? '保存中…' : '保存'}</button>
            </div>
          </div> : <header className="analysis-head">
            <div className="event-heading"><span className="event-heading-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round"><path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/></svg></span><div><h1>{event.title}</h1>
            <div className="event-heading-meta"><span>事件</span><div className="analysis-status">
              <DoneDot done={isDone(event.status)}/>
              <span>{statusLabel(event.status)}</span>
            </div><span className="analysis-meta">{[eventNotes.length ? `${eventNotes.length} 条进展` : '', eventPeople ? `${eventPeople} 人` : '', event.last_activity_at ? `最近活动 ${shortDate(event.last_activity_at)}` : ''].filter(Boolean).join(' · ')}</span></div></div></div>
            <nav className="event-section-nav" aria-label="事件详情"><button type="button" onClick={() => document.getElementById('event-overview')?.scrollIntoView({ block: 'nearest' })}>概览</button><button type="button" onClick={() => document.getElementById('event-timeline')?.scrollIntoView({ block: 'start' })}>进展与原文</button></nav>
            <section className="analysis-current-state" aria-labelledby="analysis-current-title">
              <h2 id="analysis-current-title">事件概述</h2>
              <p className="analysis-summary">{event.content || '服务端尚未返回事件内容。'}</p>
            </section>
            <section className="event-overview" id="event-overview" aria-label="事件关键信息">
              <div><span className="event-stat-icon" aria-hidden="true">◉</span><div><span>当前状态</span><strong>{statusLabel(event.status)}</strong></div></div>
              <div><span className="event-stat-icon" aria-hidden="true">≡</span><div><span>事件进展</span><strong>{eventNotes.length} 条</strong></div></div>
              <div><span className="event-stat-icon" aria-hidden="true">↗</span><div><span>关联事件</span><strong>{relatedEvents.length} 个</strong></div></div>
            </section>
            {eventTags.length > 0 && <section className="event-topics" aria-label="关联主题" key={event.id}>
              <h2>关联主题</h2><div className="event-topic-tags">{eventTags.slice(0, 8).map((tag, index) => <span className="event-topic-tag" key={`${index}-${tag}`}>{tag}</span>)}</div>
              {eventTags.length > 8 && <details className="event-topic-more"><summary>查看全部 {eventTags.length} 个</summary><div className="event-topic-tags">{eventTags.slice(8).map((tag, index) => <span className="event-topic-tag" key={`${index}-${tag}`}>{tag}</span>)}</div></details>}
            </section>}
          </header>}
          {actionError && !editing && <p className="analysis-action-error" role="alert">{actionError}</p>}

          <section className="analysis-timeline" id="event-timeline">
            <div className="analysis-block-head"><span>时间线</span><span>原始记录按需展开</span></div>
            {timelineRows.length === 0 && <p className="analysis-list-empty">服务端还没有为这个事件记录进展。</p>}
            {timelineRows.map((row, index) => {
              const last = index === timelineRows.length - 1;
              if (row.kind === 'link') {
                const relation = row.relation;
                return <div className="analysis-step analysis-step-link" key={`rel-${relation.id}`}>
                  <time/>
                  <span className="analysis-step-gutter" aria-hidden="true"><i/>{!last && <b/>}</span>
                  <div className="analysis-step-body">
                    <div className="analysis-step-tags">
                      <span className="analysis-tag">关联事件</span>
                      <span className="analysis-conv">{[relation.relation_type || relation.type || relation.label, relation.status ? statusLabel(relation.status) : ''].filter(Boolean).join(' · ')}</span>
                      {relation.relation_id && <button type="button" className="analysis-step-act" disabled={actionBusy === 'unlink'} onClick={() => void unlinkEvent(relation.relation_id!)}>取消关联</button>}
                    </div>
                    <button type="button" className="analysis-step-preview link" onClick={() => openRelatedEvent(relation.id)}>↗ {relation.title || `事件 #${relation.id}`}</button>
                  </div>
                </div>;
              }
              const note = row.note;
              const parts = noteParts(note);
              const open = !!openNotes[note.id];
              const preview = parts.messages[parts.messages.length - 1];
              return <div className={`analysis-step${note.origin === 'manual' ? ' manual' : ''}`} key={note.id}>
                <time>{clockDate(note.start_at || note.end_at)}</time>
                <span className={`analysis-step-gutter${last ? ' last' : ''}`} aria-hidden="true"><i/>{!last && <b/>}</span>
                <div className="analysis-step-body">
                  <div className="analysis-step-tags">
                    {parts.tag && <span className="analysis-tag">{parts.tag}</span>}
                    <span className="analysis-conv">{parts.conversation}</span>
                    <button type="button" className="analysis-step-act" disabled={!!actionBusy} onClick={() => void removeNote(note)}>移出此事件</button>
                  </div>
                  {preview && <p className="analysis-step-preview">{preview.text}</p>}
                  {parts.messages.length > 0 && note.origin !== 'manual' && <button
                    type="button"
                    className={`analysis-raw-toggle${open ? ' on' : ''}`}
                    onClick={() => setOpenNotes(current => ({ ...current, [note.id]: !open }))}
                  >{open ? `原文 ${parts.messages.length} 条 · 查看中` : `原文 ${parts.messages.length} 条 →`}</button>}
                  {open && <div className="analysis-raw">
                    {parts.messages.map((message, messageIndex) => <div className="analysis-raw-item" key={messageIndex}>
                      {(message.who || message.stamp) && <span>{[message.who, message.stamp].filter(Boolean).join(' · ')}</span>}
                      <p>{message.text}</p>
                    </div>)}
                  </div>}
                </div>
              </div>;
            })}
            {removedNotes.length > 0 && <div className="analysis-removed">
              <span/>
              <span/>
              <div>已移出 {removedNotes.length} 条进展（已反馈给云端）<button type="button" disabled={!!actionBusy} onClick={() => void undoRemoveNotes()}>撤销</button></div>
            </div>}
            <div className="analysis-add">
              <span/>
              <span/>
              <div>
                {!addingProgress && <button type="button" className="analysis-add-open" disabled={!!actionBusy} onClick={() => { setActionError(''); setAddingProgress(true); }}>＋ 补充进展（电话、会议等聊天之外的信息）</button>}
                {addingProgress && <div className="analysis-add-card">
                  <textarea
                    autoFocus
                    rows={2}
                    value={progressText}
                    aria-label="补充进展"
                    placeholder="例如：下午电话沟通，客户同意修复延到下周三"
                    onChange={change => setProgressText(change.target.value)}
                  />
                  <div className="analysis-add-foot">
                    <button type="button" disabled={actionBusy === 'save'} onClick={() => { setAddingProgress(false); setProgressText(''); setActionError(''); }}>取消</button>
                    <button type="button" className="primary" disabled={actionBusy === 'save'} onClick={() => void addProgress()}>{actionBusy === 'save' ? '保存中…' : '添加到时间线'}</button>
                  </div>
                </div>}
              </div>
            </div>
          </section>
        </article>}

        {report && <article className="report-doc">
          <header className="report-head">
            <span className="report-badge">{reportBadge(report)}</span>
            <div>
              <h1>{report.title}</h1>
              <p>{reportType(report)} · {report.generated_at ? `${date(report.generated_at)} 生成` : '服务端生成'}</p>
            </div>
          </header>

          <div className="report-summary">
            <div className="report-stats">
              {(report.period_start || report.period_end) && <span>周期 {date(report.period_start)} – {date(report.period_end)}</span>}
              <span>{report.sections?.length ?? 0} 个分类 · {sectionItems(report).length} 项</span>
            </div>
            <p>{['queued', 'processing', 'partial'].includes(report.status || '')
              ? '服务端正在生成这份报告，稍后刷新即可看到内容。'
              : reportContent}</p>
          </div>

          {(report.sections ?? []).map(section => <section className="report-section" key={section.id}>
            <h3>{section.title}{section.items?.length ? ` · ${section.items.length}` : ''}</h3>
            <div className="report-items">
              {(section.items ?? []).map(item => {
                const eventSource = item.sources?.find(source => source.event_id)?.event_id;
                const noteSource = item.sources?.find(source => source.note_id)?.note_id;
                const target: EvidenceTarget | null = eventSource
                  ? { kind: 'event', id: eventSource }
                  : noteSource ? { kind: 'note', id: noteSource } : null;
                return <button
                  type="button"
                  className="report-item"
                  key={item.id}
                  onClick={() => target && setEvidence(target)}
                  disabled={!target}
                >
                  <span className="report-item-dot" aria-hidden="true"/>
                  <span className="report-item-body">
                    <strong>{item.title}</strong>
                    {(item.summary || item.time_text || item.contact_text || item.followup_text) && <small>
                      {[item.summary, item.time_text, item.contact_text, item.followup_text].filter(Boolean).join(' · ')}
                    </small>}
                  </span>
                  {item.label && <span className="report-item-label">{item.label}</span>}
                </button>;
              })}
            </div>
          </section>)}

          <p className="report-note">每一项都是事件，点击可在右侧预览；需要完整时间线时再打开详情。</p>
        </article>}
        </>}

      </div>
    </main>

    {pickerOpen && event && <div className="analysis-picker-backdrop" role="presentation" onClick={() => setPickerOpen(false)}>
      <div className="analysis-picker" role="dialog" aria-modal="true" aria-label="关联事件" onClick={stop => stop.stopPropagation()}>
        <header>
          <span>关联到「{event.title}」</span>
          <input autoFocus value={pickerQuery} placeholder="搜索事件…" aria-label="搜索可关联的事件" onChange={change => setPickerQuery(change.target.value)}/>
        </header>
        <div className="analysis-picker-list">
          {linkCandidates.map(item => <div className="analysis-picker-row" key={item.id}>
            <DoneDot done={isDone(item.status)}/>
            <span>
              <strong>{item.title || `事件 #${item.id}`}</strong>
              <small>{[statusLabel(item.status), item.last_activity_at ? shortDate(item.last_activity_at) : ''].filter(Boolean).join(' · ')}</small>
            </span>
            <button type="button" disabled={actionBusy === 'link'} onClick={() => void linkEvent(item.id)}>关联</button>
          </div>)}
          {!linkCandidates.length && <p className="analysis-list-empty">没有可关联的事件</p>}
        </div>
        <footer>关联：两件事相关，但各自独立跟踪。你的操作会反馈给云端，之后的新消息按此归类。</footer>
      </div>
    </div>}

    {evidence && <EvidenceDrawer
      spaceId={spaceId}
      target={evidence}
      onClose={() => setEvidence(null)}
      onNavigate={setEvidence}
      onOpenDetail={target => { if (target.kind === 'event') onOpenEvent?.(target.id); }}
    />}
  </div>;
}
