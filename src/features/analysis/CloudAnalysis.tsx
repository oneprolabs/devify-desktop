import { t, tf, ctf, dateLocale, sortLocale } from '../../shared/i18n/locale';
import { LoadingPage, LoadingSkeleton } from '../../shared/ui/LoadingSkeleton';
import { SidebarResizeHandle } from '../../shared/ui/SidebarResizeHandle';
import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { EvidenceDrawer, type EvidenceTarget } from '../../shared/ui/EvidenceDrawer';
import { reportPresentation } from './reportPresentation';
import { renderAgentMarkdown } from '../../shared/content/summary';
import { readPageSnapshot, savePageSnapshot, clearPageSnapshots, pageSnapshotGeneration } from '../../shared/state/pageSnapshots';

type Kind = 'events' | 'reports';
type EventItem = { id: number; title: string; content: string; status?: string | null; tags?: Array<{ label?: string }>; last_activity_at?: string | null; opened_at?: string | null; updated_at?: string | null };
type RelatedEvent = { id: number; title?: string; status?: string | null; relation_type?: string; type?: string; label?: string; relation_id?: number };
type EventRelation = Omit<RelatedEvent, 'id'> & { id?: number; event_id?: number; related_event_id?: number; source_event_id?: number; target_event_id?: number };
type EventRelationRecord = { id: number; space_id: string; left_event_id: number; right_event_id: number; relation_kind: string; decision: string; reason?: string };
type ReportSection = { id: number; title: string; position: number; items?: Array<{ id: number; title: string; summary: string; label?: string; time_text?: string; contact_text?: string; followup_text?: string; sources?: Array<{ display_text?: string; event_id?: number | null; note_id?: number | null }> }> };
type ReportItem = { id: number; title: string; body?: string; content?: unknown; status: string; period_start?: string | null; period_end?: string | null; generated_at?: string | null; updated_at?: string | null; sections?: ReportSection[] };
type EventNote = { id: number; title: string; content: string; start_at?: string | null; end_at?: string | null; origin?: string | null };
type EventDetail = EventItem & {
  evidence?: Array<{ id: number; sender_name?: string; occurred_at?: string | null; content?: string }>;
  note_ids?: number[] | string;
  relation_ids?: number[] | string;
  related_event_ids?: number[];
  relations?: EventRelation[];
  related_events?: EventRelation[];
};
type Page<T> = { list: T[]; pagination?: { total?: number; page?: number; next?: string | null } };
type AppSettings = { devify?: { base_url?: string; active_space_id: string | null } };

const labels: Record<Kind, { title: string; empty: string }> = {
  events: { title: '待办', empty: '当前空间还没有待办。' },
  reports: { title: '报告', empty: '当前空间还没有报告。' },
};
const REPORT_TYPES = [['all', '全部'], ['日报', '日报'], ['周报', '周报'], ['月报', '月报'], ['主题', '主题']];

function date(value?: string | null): string {
  if (!value) return t("时间未知");
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(dateLocale(), { dateStyle: 'medium', timeStyle: 'short' });
}
function shortDate(value?: string | null): string {
  if (!value) return '';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(dateLocale(), { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
function clockDate(value?: string | null): string {
  if (!value) return '—';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString(dateLocale(), { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
const RELATION_KINDS: Record<string, string> = { same: '同一件事', related: '相关', depends_on: '依赖', separate: '无关' };
function relationKindLabel(kind?: string | null): string {
  return RELATION_KINDS[kind ?? ''] ?? t("关联");
}
function statusLabel(value?: string | null): string {
  return ({ open: t("进行中"), in_progress: t("推进中"), done: t("已完成"), queued: t("排队中"), ready: t("已生成"), partial: t("部分生成"), failed: t("生成失败") } as Record<string, string>)[value || ''] || value || t("未标记");
}
/** Notes arrive as `［会话标签］会话名  #消息区间`; split the tag off and drop the
 *  message-range suffix, which is ingestion bookkeeping rather than a name. */
function noteParts(note: EventNote): { tag: string; conversation: string; lines: string[]; messages: RawMessage[] } {
  const match = /^[［\[]([^］\]]+)[］\]]\s*(.*)$/.exec(note.title ?? '');
  const lines = (note.content ?? '').split('\n').map(line => line.trim()).filter(Boolean);
  const name = ((match ? match[2] : note.title) || tf("记录 #{0}", note.id)).replace(/\s*#\d+\s*-\s*\d+\s*$/, '').trim();
  return {
    tag: match ? match[1] : '',
    conversation: name || tf("记录 #{0}", note.id),
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
    return { start, end: now, hint: tf("本周 · {0} – {1}", fmt(start), fmt(end)), title: tf("周报 · 第 {0} 周", isoWeek(now)) };
  }
  if (type === '月报') {
    start.setDate(1);
    start.setHours(0, 0, 0, 0);
    return { start, end: now, hint: tf("本月 · {0} – {1}", fmt(start), fmt(now)), title: tf("月报 · {0} 月", now.getMonth() + 1) };
  }
  start.setHours(0, 0, 0, 0);
  return { start, end: now, hint: tf("今天 · {0}", fmt(now)), title: tf("日报 · {0}", fmt(now)) };
}
function fmt(date: Date): string {
  return date.toLocaleDateString(dateLocale(), { month: '2-digit', day: '2-digit' });
}
function isoWeek(date: Date): number {
  const target = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const day = target.getUTCDay() || 7;
  target.setUTCDate(target.getUTCDate() + 4 - day);
  const yearStart = new Date(Date.UTC(target.getUTCFullYear(), 0, 1));
  return Math.ceil((((target.getTime() - yearStart.getTime()) / 86400000) + 1) / 7);
}

function reportKind(report: ReportItem): string {
  if (report.title.includes("周报")) return "周报";
  if (report.title.includes("月报")) return "月报";
  if (report.title.includes("日报")) return "日报";
  return "报告";
}
function reportType(report: ReportItem): string {
  return t(reportKind(report));
}
function reportBadge(report: ReportItem): string {
  return { 日报: t("日"), 周报: t("周"), 月报: t("月") }[reportKind(report)] ?? t("报");
}
function sectionItems(report: ReportItem): string[] {
  return (report.sections ?? []).flatMap(section => (section.items ?? []).map(item => item.title));
}

export function CloudAnalysisPage({ kind, userId, focusId, onOpenEvent, onOpenAnalysisSettings }: { kind: Kind; userId: number | null; focusId?: number | null; onOpenEvent?: (eventId: number) => void; onOpenAnalysisSettings?: () => void }) {
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [items, setItems] = useState<Array<EventItem | ReportItem>>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [loading, setLoading] = useState(isTauri());
  const [error, setError] = useState('');
  const [listError, setListError] = useState('');
  const [total, setTotal] = useState(0);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState(kind === 'events' ? 'open' : 'all');
  const [eventTab, setEventTab] = useState<'overview' | 'sources'>('overview');
  const [detail, setDetail] = useState<EventDetail | ReportItem | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [eventNotes, setEventNotes] = useState<EventNote[]>([]);
  const [openNotes, setOpenNotes] = useState<Record<number, boolean>>({});
  const [reloadKey, setReloadKey] = useState(0);
  const [creating, setCreating] = useState(false);
  const [genType, setGenType] = useState("日报");
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
    let cacheKey = '';
    const generation = pageSnapshotGeneration();
    const initialSelection = (list: Array<EventItem | ReportItem>) => focusId
      ?? (kind === 'events' ? list.find(item => !isDone((item as EventItem).status))?.id : list[0]?.id)
      ?? null;
    setItems([]); setSelectedId(null); setDetail(null); setError(''); setListError(''); setQuery(''); setFilter(kind === 'events' && focusId == null ? 'open' : 'all');
    setRelations([]); setEditing(false); setPickerOpen(false); setActionError(''); setActionBusy(null); setRemovedNotes([]); setAddingProgress(false); setProgressText('');
    if (!isTauri()) { setLoading(false); return; }
    setLoading(true);
    void invoke<AppSettings>('app_settings_get')
      .then(async settings => {
        if (!active) return;
        const id = settings.devify?.active_space_id ?? await invoke<string | null>('devify_space_ensure');
        if (!active) return;
        setSpaceId(id);
        if (!id) throw new Error(t("暂时无法读取空间，请检查账户连接后重试。"));
        cacheKey = userId === null ? '' : JSON.stringify([settings.devify?.base_url, userId, id, kind]);
        const cached = cacheKey ? readPageSnapshot<{ list: Array<EventItem | ReportItem>; total: number }>(cacheKey) : undefined;
        if (cached) {
          setItems(cached.list);
          setTotal(cached.total);
          setSelectedId(initialSelection(cached.list));
          setLoading(false);
        }
        return invoke<Page<EventItem> | Page<ReportItem>>('devify_analysis_list', { kind, spaceId: id, page: 1 });
      })
      .then(result => {
        if (!active || !result) return;
        const list = result.list ?? [];
        if (cacheKey) savePageSnapshot(cacheKey, { list, total: result.pagination?.total ?? list.length }, generation);
        setItems(list);
        setTotal(result.pagination?.total ?? list.length);
        const pending = pendingSelect.current;
        pendingSelect.current = null;
        setSelectedId(current => pending && list.some(item => item.id === pending)
          ? pending
          : focusId && (kind === 'events' || list.some(item => item.id === focusId)) ? focusId
          : current && list.some(item => item.id === current) ? current : initialSelection(list));
      })
      .catch(reason => { if (active) setListError(String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [kind, userId, focusId, reloadKey]);

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
  useEffect(() => {
    const focused = focusId != null && selectedId === focusId;
    if (kind === 'events' && items.length && !focused && !filteredItems.some(item => item.id === selectedId)) {
      setSelectedId(filteredItems[0]?.id ?? null);
    }
  }, [filteredItems, selectedId, kind, items.length, focusId]);
  const selected = useMemo(() => items.find(item => item.id === selectedId) ?? null, [items, selectedId]);

  useEffect(() => {
    if (!isTauri() || !spaceId || selectedId == null) { setDetail(null); setEventNotes([]); setDetailLoading(false); setError(''); return; }
    let active = true;
    setDetailLoading(true);
    setError('');
    setOpenNotes({});
    setEventTab('overview');
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

  const currentDetail = detail?.id === selectedId ? detail : null;
  const report = !detailLoading && kind === 'reports' && (currentDetail || selected) ? (currentDetail || selected) as ReportItem : null;
  const event = !detailLoading && kind === 'events' && (currentDetail || selected) ? (currentDetail || selected) as EventDetail : null;
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
    ...[...eventNotes].reverse().map(note => ({ kind: 'note' as const, note })),
    ...relatedEvents.map(relation => ({ kind: 'link' as const, relation })),
  ], [eventNotes, relatedEvents]);
  const allRawOpen = eventNotes.length > 0 && eventNotes.every(note => openNotes[note.id]);
  const toggleAllRaw = () => setOpenNotes(allRawOpen ? {} : Object.fromEntries(eventNotes.map(note => [note.id, true])));
  const reportView = reportPresentation(report ?? {});
  const applyEventUpdate = (updated: EventItem) => {
    clearPageSnapshots();
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
    clearPageSnapshots();
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
      setActionError(t("请先填写进展内容。"));
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
    clearPageSnapshots();
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
      if (!items.some(item => item.id === eventId)) setItems(current => [{ id: eventId, title: tf("待办 #{0}", eventId), content: '' }, ...current]);
      setSelectedId(eventId);
    } else onOpenEvent?.(eventId);
  };
  const generateReport = async () => {
    clearPageSnapshots();
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
    ? (filter === 'open' ? t("待跟进") : filter === 'done' ? t("已完成") : t("全部"))
    : t(filter === 'all' ? "全部" : filter);
  const filterPills: Array<[string, string, number]> = kind === 'events'
    ? [['open', t("待跟进"), counts.open], ['done', t("已完成"), counts.done], ['all', t("全部"), items.length]]
    : REPORT_TYPES.map(([value, label]) => [value, label, value === 'all' ? items.length : items.filter(item => reportKind(item as ReportItem) === value).length]);

  const pageTitle = t(copy.title);
  const pageEmpty = t(copy.empty);
  if (!isTauri()) return <div className="analysis-page"><div className="analysis-empty"><h2>{t("请在桌面应用中查看")}</h2><p>{t("待办和报告在上传处理后生成，桌面端负责同步并展示。")}</p></div></div>;
  if (loading) return <LoadingPage title={pageTitle} />;
  if (error || (listError && !items.length)) return <div className="analysis-page"><div className="analysis-empty error" role="alert"><h2>{t("同步失败")}</h2><p>{error || listError}</p></div></div>;

  return <div className={`analysis-page${kind === 'events' ? ' event-workspace' : ''}`}>
    <aside className="analysis-side" aria-label={tf("{0}列表", pageTitle)}>
      <h1>{pageTitle}</h1>
      {listError && <p role="status">{t("暂时无法更新，显示上次读取的内容。")}<button type="button" onClick={() => setReloadKey(key => key + 1)}>{t("重试")}</button></p>}
      <p className="analysis-side-count">{kind === 'events' ? tf("{0} 个未完成 · 跨全部会话", counts.open) : tf("{0} 份报告", items.length)}</p>

      <div className="analysis-search">
        <input value={query} spellCheck={false} placeholder={tf("搜索{0}", pageTitle)} aria-label={tf("搜索{0}", pageTitle)} onChange={change => setQuery(change.target.value)}/>
      </div>

      {kind === 'reports' && <button type="button" className={`analysis-side-new${creating ? ' on' : ''}`} onClick={() => { setCreating(true); setGenError(''); }}>{t("＋ 生成报告")}</button>}

      <div className="analysis-side-row">
        <span>{pageTitle}</span>
        <button type="button" onClick={cycleStatus}>{filterLabel} ▾</button>
      </div>

      <div className="analysis-pills" role="group" aria-label={t("筛选")}>
        {filterPills.map(([value, label, count]) => <button
          key={value}
          type="button"
          aria-pressed={filter === value}
          className={filter === value ? 'on' : ''}
          onClick={() => setFilter(value)}
        >{t(label)}<small>{count}</small></button>)}
      </div>

      <div className="analysis-items">
        {filteredItems.map(item => {
          const status = kind === 'events' ? (item as EventItem).status : (item as ReportItem).status;
          const when = kind === 'events' ? (item as EventItem).last_activity_at : (item as ReportItem).generated_at;
          return <button key={item.id}
            type="button"
            aria-current={item.id === selectedId ? 'true' : undefined}
            className={item.id === selectedId ? 'on' : ''}
            onClick={() => { setCreating(false); setSelectedId(item.id); }}
          >
            {kind === 'reports'
              ? <span className="report-mini-badge">{reportBadge(item as ReportItem)}</span>
              : <span className="event-list-icon" aria-hidden="true"><DoneDot done={isDone(status)}/></span>}
            <span>
              <strong>{item.title || `${pageTitle} #${item.id}`}</strong>
              {kind === 'events' ? <small className="event-list-meta"><span>{status === 'open' ? t("待跟进") : statusLabel(status)}</span>{when && <time dateTime={when}>{shortDate(when)}</time>}</small>
                : <small>{reportType(item as ReportItem)}{when ? ` · ${shortDate(when)}` : ''}</small>}
            </span>
            </button>;
        })}
        {!filteredItems.length && <p className="analysis-list-empty">{t("没有符合筛选条件的")}{pageTitle}</p>}
      </div>
      {kind === 'events' && <SidebarResizeHandle/>}
    </aside>

    <main className="analysis-main">
      <header className="analysis-top">
        <div className="analysis-crumbs"><span className="analysis-crumb-root">{pageTitle}</span><span className="analysis-crumb-sep">/</span><span className="analysis-crumb-current">{filterLabel}</span></div>
        <div className="analysis-top-actions">
          {kind === 'events' && event && <>
            <button type="button" disabled={!!actionBusy} onClick={startEdit}>{t("编辑")}</button>
            <button type="button" disabled={!!actionBusy} onClick={() => { setActionError(''); setPickerQuery(''); setPickerOpen(true); }}>{t("关联待办")}</button>
            {((event.evidence?.length ?? 0) > 0 || eventNotes.length > 0) && <button type="button" onClick={() => setEventTab('sources')}>{t("查看全部原文")}</button>}
            <button
              type="button"
              className={isDone(event.status) ? '' : 'primary'}
              disabled={actionBusy === 'status'}
              onClick={() => void toggleEventStatus()}
            >{actionBusy === 'status' ? t("保存中…") : isDone(event.status) ? t("重新打开") : t("标记完成")}</button>
          </>}
          <button type="button" onClick={() => setReloadKey(key => key + 1)}>{t("刷新")}</button>
        </div>
      </header>

      <div className="analysis-doc-wrap">
        {kind==='reports'&&!creating&&!report&&!detailLoading&&<div className="analysis-empty"><h2>{t('还没有报告')}</h2><p>{t('可以按需生成第一份报告，也可以在智能分析中设置定期生成。')}</p><button type="button" className="report-create-submit" onClick={()=>setCreating(true)}>{t('生成报告')}</button></div>}
        {kind === 'reports' && creating && <div className="report-create">
          <header>
            <h2>{t("生成报告")}</h2>
            <p>{t("日报、周报、月报会按时自动生成；这里可以随时按需补生成一份。")}</p>
          </header>
          <div className="report-create-grid">
            <span className="report-create-label">{t("类型")}</span>
            <div className="report-create-field">
              <div className="report-seg" role="group" aria-label={t("报告类型")}>
                {["日报", "周报", "月报"].map(type => <button
                  key={type}
                  type="button"
                  aria-pressed={genType === type}
                  className={genType === type ? 'on' : ''}
                  onClick={() => setGenType(type)}
                >{t(type)}</button>)}
              </div>
              <small>{t(genRange(genType).hint)}</small>
            </div>
          </div>
          {genError && <p className="analysis-empty error" role="alert">{genError}</p>}
          <button type="button" className="report-create-submit" disabled={generating} onClick={() => void generateReport()}>{generating ? t("正在提交…") : t("生成")}</button>
          <p className="report-create-note">{t("会按所选区间重新汇总待办；生成完成后会出现在左侧列表，标题为「")}{genRange(genType).title}」。</p>
        </div>}

        {!(kind === 'reports' && creating) && <>
        {detailLoading && <LoadingSkeleton label={t("正在加载详情")} kind="detail" />}
        {kind === 'events' && selectedId == null && !detailLoading && <div className="analysis-empty event-empty" role="status">
          <h2>{items.length ? t("当前筛选下没有记录。") : pageEmpty}</h2>
          <p>{items.length ? t("可切换筛选查看其他待办。") : t("已备份的聊天大约每小时分析一次，生成的待办会显示在这里。可在「智能分析 → 聊天待办分析」中调整设置。")}</p>
          {items.length ? <button type="button" className="event-empty-action" onClick={() => { setQuery(''); setFilter('all'); }}>{t("查看全部待办")}</button>
            : onOpenAnalysisSettings && <button type="button" className="event-empty-action" onClick={onOpenAnalysisSettings}>{t("设置聊天待办分析")}</button>}
        </div>}

        {event && <article className="analysis-doc">
          {editing ? <div className="analysis-edit">
            <input
              className="analysis-edit-title"
              value={editTitle}
              aria-label={t("待办标题")}
              onChange={change => setEditTitle(change.target.value)}
            />
            <label className="analysis-edit-label" htmlFor="event-edit-summary">{t("摘要")}</label>
            <textarea
              id="event-edit-summary"
              value={editContent}
              rows={4}
              onChange={change => setEditContent(change.target.value)}
            />
            {actionError && <p className="analysis-action-error" role="alert">{actionError}</p>}
            <div className="analysis-edit-foot">
              <span>{t("保存后以你的版本为准，云端不会覆盖")}</span>
              <button type="button" disabled={actionBusy === 'save'} onClick={() => { setEditing(false); setActionError(''); }}>{t("取消")}</button>
              <button type="button" className="primary" disabled={actionBusy === 'save'} onClick={() => void saveEventEdit()}>{actionBusy === 'save' ? t("保存中…") : t("保存")}</button>
            </div>
          </div> : <header className="analysis-head">
            <div className="event-heading"><span className="event-heading-icon" aria-hidden="true"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinejoin="round"><path d="m12 3 9 5-9 5-9-5 9-5Z"/><path d="m3 12 9 5 9-5M3 16l9 5 9-5"/></svg></span><div><h1>{event.title}</h1>
            <div className="event-heading-meta"><div className="analysis-status">
              <DoneDot done={isDone(event.status)}/>
              <span>{event.status === 'open' ? t("待跟进") : statusLabel(event.status)}</span>
            </div><span className="analysis-meta">{[eventNotes.length ? tf("{0} 条进展", eventNotes.length) : '', eventPeople ? tf("{0} 人", eventPeople) : '', event.last_activity_at ? tf("最近活动 {0}", shortDate(event.last_activity_at)) : ''].filter(Boolean).join(' · ')}</span></div></div></div>
            <nav className="event-section-nav" aria-label={t("待办详情")}>
              <button type="button" aria-pressed={eventTab === 'overview'} onClick={() => setEventTab('overview')}>{t("概览")}</button>
              <button type="button" aria-pressed={eventTab === 'sources'} onClick={() => setEventTab('sources')}>{t("进展与原文")}</button>
            </nav>
            {eventTab === 'overview' && <section className="analysis-current-state" aria-labelledby="analysis-current-title">
              <h2 id="analysis-current-title">{t("待办概述")}</h2>
              <p className="analysis-summary">{event.content || t("尚未返回待办内容。")}</p>
            </section>}
          </header>}
          {actionError && !editing && <p className="analysis-action-error" role="alert">{actionError}</p>}

          <section className="analysis-timeline" id="event-timeline">
            <div className="analysis-block-head"><h2>{t("时间线")}</h2>{eventTab === 'sources' && eventNotes.length > 0 && !event.evidence?.length && <button type="button" className="analysis-raw-toggle" onClick={toggleAllRaw}>{allRawOpen ? t("收起全部原文") : t("查看全部原文")}</button>}</div>
            {eventTab === 'sources' && !!event.evidence?.length && <div className="analysis-raw event-source-list">
              {event.evidence.map(source => <div className="analysis-raw-item" key={source.id}>
                <span>{[source.sender_name, shortDate(source.occurred_at)].filter(Boolean).join(' · ')}</span>
                <p>{source.content}</p>
              </div>)}
            </div>}
            {timelineRows.length === 0 && !(eventTab === 'sources' && event.evidence?.length) && <p className="analysis-list-empty">{t("还没有为这个待办记录进展。")}</p>}
            {timelineRows.map((row, index) => {
              const last = index === timelineRows.length - 1;
              if (row.kind === 'link') {
                const relation = row.relation;
                return <div className="analysis-step analysis-step-link" key={`rel-${relation.id}`}>
                  <time/>
                  <span className="analysis-step-gutter" aria-hidden="true"><i/>{!last && <b/>}</span>
                  <div className="analysis-step-body">
                    <div className="analysis-step-tags">
                      <span className="analysis-tag">{t("关联待办")}</span>
                      <span className="analysis-conv">{[relation.relation_type || relation.type || relation.label, relation.status ? statusLabel(relation.status) : ''].filter(Boolean).join(' · ')}</span>
                      {relation.relation_id && <button type="button" className="analysis-step-act" disabled={actionBusy === 'unlink'} onClick={() => void unlinkEvent(relation.relation_id!)}>{t("取消关联")}</button>}
                    </div>
                    <button type="button" className="analysis-step-preview link" onClick={() => openRelatedEvent(relation.id)}>↗ {relation.title || tf("待办 #{0}", relation.id)}</button>
                  </div>
                </div>;
              }
              const note = row.note;
              const parts = noteParts(note);
              const open = !!openNotes[note.id];
              return <div className={`analysis-step${note.origin === 'manual' ? ' manual' : ''}`} key={note.id}>
                <time>{clockDate(note.start_at || note.end_at)}</time>
                <span className={`analysis-step-gutter${last ? ' last' : ''}`} aria-hidden="true"><i/>{!last && <b/>}</span>
                <div className="analysis-step-body">
                  <div className="analysis-step-tags">
                    {parts.tag && <span className="analysis-tag">{parts.tag}</span>}
                    <span className="analysis-conv">{parts.conversation}</span>
                    <button type="button" className="analysis-step-act" disabled={!!actionBusy} onClick={() => void removeNote(note)}>{t("移出此待办")}</button>
                  </div>
                  {note.origin === 'manual' && <p className="analysis-step-preview">{note.content}</p>}
                  {parts.messages.length > 0 && note.origin !== 'manual' && <button
                    type="button"
                    className={`analysis-raw-toggle${open ? ' on' : ''}`}
                    aria-expanded={open}
                    aria-controls={`event-note-raw-${note.id}`}
                    onClick={() => setOpenNotes(current => ({ ...current, [note.id]: !open }))}
                  >{open ? t("收起原文") : tf("展开原文 {0} 条", parts.messages.length)}</button>}
                  {open && <div className="analysis-raw" id={`event-note-raw-${note.id}`}>
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
              <div>{t("已移出")} {removedNotes.length} {t("条进展（已反馈给云端）")}<button type="button" disabled={!!actionBusy} onClick={() => void undoRemoveNotes()}>{t("撤销")}</button></div>
            </div>}
            <div className="analysis-add">
              <span/>
              <span/>
              <div>
                {!addingProgress && <button type="button" className="analysis-add-open" disabled={!!actionBusy} onClick={() => { setActionError(''); setAddingProgress(true); }}>{t("＋ 补充进展（电话、会议等聊天之外的信息）")}</button>}
                {addingProgress && <div className="analysis-add-card">
                  <textarea
                    autoFocus
                    rows={2}
                    value={progressText}
                    aria-label={t("补充进展")}
                    placeholder={t("例如：下午电话沟通，客户同意修复延到下周三")}
                    onChange={change => setProgressText(change.target.value)}
                  />
                  <div className="analysis-add-foot">
                    <button type="button" disabled={actionBusy === 'save'} onClick={() => { setAddingProgress(false); setProgressText(''); setActionError(''); }}>{t("取消")}</button>
                    <button type="button" className="primary" disabled={actionBusy === 'save'} onClick={() => void addProgress()}>{actionBusy === 'save' ? t("保存中…") : t("添加到时间线")}</button>
                  </div>
                </div>}
              </div>
            </div>
          </section>
            {eventTags.length > 0 && <section className="event-topics" aria-label={t("关联主题")} key={event.id}>
              <h2>{t("关联主题")}</h2><div className="event-topic-tags">{eventTags.slice(0, 8).map((tag, index) => <span className="event-topic-tag" key={`${index}-${tag}`}>{tag}</span>)}</div>
              {eventTags.length > 8 && <details className="event-topic-more"><summary>{t("查看全部")} {eventTags.length} {t("个")}</summary><div className="event-topic-tags">{eventTags.slice(8).map((tag, index) => <span className="event-topic-tag" key={`${index}-${tag}`}>{tag}</span>)}</div></details>}
            </section>}
        </article>}

        {report && <article className="report-doc">
          <header className="report-head">
            <span className="report-badge">{reportBadge(report)}</span>
            <div>
              <h1>{report.title}</h1>
              <p>{reportType(report)} · {report.generated_at ? tf("{0} 生成", date(report.generated_at)) : statusLabel(report.status)}</p>
            </div>
          </header>

          <div className="report-summary">
            <div className="report-stats">
              {(report.period_start || report.period_end) && <span>{t("周期")} {date(report.period_start)} – {date(report.period_end)}</span>}
              <span>{report.sections?.length ?? 0} {t("个分类 ·")} {sectionItems(report).length} {t("项")}</span>
            </div>
            {reportView.notice && <p role={report.status === 'failed' ? 'alert' : 'status'}>{reportView.notice}</p>}
            {reportView.content ? <div className="report-markdown wxv-ai-body">{renderAgentMarkdown(reportView.content)}</div> : !reportView.notice && <p>{t("尚未返回正文。")}</p>}
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
                  {item.label && <span className="report-item-label">{t(item.label)}</span>}
                </button>;
              })}
            </div>
          </section>)}

          <p className="report-note">{t("每一项都是待办，点击可在右侧预览；需要完整时间线时再打开详情。")}</p>
        </article>}
        </>}

      </div>
    </main>

    {pickerOpen && event && <div className="analysis-picker-backdrop" role="presentation" onClick={() => setPickerOpen(false)}>
      <div className="analysis-picker" role="dialog" aria-modal="true" aria-label={t("关联待办")} onClick={stop => stop.stopPropagation()}>
        <header>
          <span>{t("关联到「")}{event.title}」</span>
          <input autoFocus value={pickerQuery} placeholder={t("搜索待办…")} aria-label={t("搜索可关联的待办")} onChange={change => setPickerQuery(change.target.value)}/>
        </header>
        <div className="analysis-picker-list">
          {linkCandidates.map(item => <div className="analysis-picker-row" key={item.id}>
            <DoneDot done={isDone(item.status)}/>
            <span>
              <strong>{item.title || tf("待办 #{0}", item.id)}</strong>
              <small>{[statusLabel(item.status), item.last_activity_at ? shortDate(item.last_activity_at) : ''].filter(Boolean).join(' · ')}</small>
            </span>
            <button type="button" disabled={actionBusy === 'link'} onClick={() => void linkEvent(item.id)}>{t("关联")}</button>
          </div>)}
          {!linkCandidates.length && <p className="analysis-list-empty">{t("没有可关联的待办")}</p>}
        </div>
        <footer>{t("关联：两件事相关，但各自独立跟踪。你的操作会反馈给云端，之后的新消息按此归类。")}</footer>
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
