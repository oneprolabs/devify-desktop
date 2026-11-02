import { useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Kind = 'events' | 'reports';
type EventItem = { id: number; title: string; content: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };
type RelatedEvent = { id: number; title?: string; status?: string | null; relation_type?: string; type?: string; label?: string };
type EventRelation = Omit<RelatedEvent, 'id'> & { id?: number; event_id?: number; related_event_id?: number; source_event_id?: number; target_event_id?: number };
type ReportSection = { id: number; title: string; position: number; items?: Array<{ id: number; title: string; summary: string; label?: string; time_text?: string; contact_text?: string; followup_text?: string; sources?: Array<{ display_text?: string; event_id?: number | null; note_id?: number | null }> }> };
type ReportItem = { id: number; title: string; body?: string; content?: unknown; status: string; period_start?: string | null; period_end?: string | null; generated_at?: string | null; updated_at?: string | null; sections?: ReportSection[] };
type EventNote = { id: number; title: string; content: string; start_at?: string | null; end_at?: string | null };
type EventDetail = EventItem & { note_ids?: number[]; relation_ids?: number[]; related_event_ids?: number[]; relations?: EventRelation[]; related_events?: EventRelation[] };
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
function statusLabel(value?: string | null): string {
  return ({ open: '进行中', in_progress: '推进中', done: '已完成', queued: '排队中', ready: '已生成', partial: '部分生成', failed: '生成失败' } as Record<string, string>)[value || ''] || value || '未标记';
}
function relationLabel(relation: RelatedEvent): string {
  return relation.relation_type || relation.type || relation.label || '关联事件';
}

function DoneDot({ done }: { done: boolean }) {
  return <svg className="analysis-dot" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25" fill={done ? '#1a1a19' : 'transparent'} stroke={done ? '#1a1a19' : '#b8b8b2'} strokeWidth={1.5}/>
    {done && <path d="M5 8.3l2.1 2.1L11 6.1" stroke="#fff" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
  </svg>;
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
  const copy = labels[kind];

  useEffect(() => {
    let active = true;
    setItems([]); setSelectedId(null); setDetail(null); setError(''); setQuery(''); setFilter('all');
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
        setSelectedId(focusId && (kind === 'events' || list.some(item => item.id === focusId)) ? focusId : list[0]?.id ?? null);
      })
      .catch(reason => { if (active) setError(String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [kind, focusId]);

  const filteredItems = useMemo(() => items.filter(item => {
    const text = `${item.title} ${kind === 'events' ? (item as EventItem).content : ''}`.toLowerCase();
    if (query.trim() && !text.includes(query.trim().toLowerCase())) return false;
    if (kind === 'events') {
      if (filter === 'open') return (item as EventItem).status !== 'done';
      if (filter === 'done') return (item as EventItem).status === 'done';
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
      .then(notes => { if (active) setEventNotes(notes ?? []); })
      .catch(() => { if (active) setEventNotes([]); });
    return () => { active = false; };
  }, [kind, spaceId, selectedId]);

  const report = kind === 'reports' && (detail || selected) ? (detail || selected) as ReportItem : null;
  const event = kind === 'events' && (detail || selected) ? (detail || selected) as EventDetail : null;
  const relatedEvents = useMemo(() => {
    if (!event) return [];
    const known = event.relations ?? event.related_events ?? [];
    const ids = event.related_event_ids ?? [];
    const byId = new Map<number, RelatedEvent>();
    known.forEach(item => {
      const id = item.event_id ?? item.related_event_id ?? item.target_event_id ?? item.source_event_id ?? item.id;
      if (id && id !== event.id) byId.set(id, { ...item, id });
    });
    ids.forEach(id => { if (id !== event.id && !byId.has(id)) byId.set(id, { id }); });
    return Array.from(byId.values());
  }, [event]);
  const reportContent = report?.body || (typeof report?.content === 'string' ? report.content : report?.content ? JSON.stringify(report.content, null, 2) : '服务端尚未返回正文。');
  const openRelatedEvent = (eventId: number) => {
    if (kind === 'events') {
      if (!items.some(item => item.id === eventId)) setItems(current => [{ id: eventId, title: `事件 #${eventId}`, content: '' }, ...current]);
      setSelectedId(eventId);
    } else onOpenEvent?.(eventId);
  };
  const exportMarkdown = () => {
    if (!report) return;
    const sections = report.sections?.map(section => `## ${section.title}\n\n${(section.items ?? []).map(item => `### ${item.title}\n\n${item.summary}`).join('\n\n')}`).join('\n\n') ?? '';
    const markdown = `# ${report.title}\n\n${reportContent}\n\n${sections}`;
    const link = document.createElement('a'); link.href = URL.createObjectURL(new Blob([markdown], { type: 'text/markdown;charset=utf-8' })); link.download = `${report.title || 'Devify报告'}.md`; link.click(); URL.revokeObjectURL(link.href);
  };
  const cycleStatus = () => setFilter(current => current === 'all' ? 'open' : current === 'open' ? 'done' : 'all');

  if (!isTauri()) return <div className="analysis-page"><div className="analysis-empty"><h2>请在桌面应用中查看</h2><p>事件和报告由 Devify 服务端生成，桌面端只负责同步并展示。</p></div></div>;
  if (loading) return <div className="analysis-page"><div className="analysis-empty"><h2>正在同步{copy.title}…</h2><p>正在从当前 Devify 空间读取最新结果。</p></div></div>;
  if (error) return <div className="analysis-page"><div className="analysis-empty error" role="alert"><h2>同步失败</h2><p>{error}</p></div></div>;
  if (!items.length) return <div className="analysis-page"><div className="analysis-empty"><h2>{copy.empty}</h2><p>完成聊天采集并等待服务端处理后，结果会出现在这里。</p></div></div>;

  const isDone = (status?: string | null) => status === 'done';

  return <div className="analysis-page">
    <aside className="analysis-side" aria-label={`${copy.title}列表`}>
      <header className="analysis-side-head">
        <span>{copy.title}</span>
        <button type="button" onClick={cycleStatus} title="切换筛选">
          {filter === 'all' ? '全部' : filter === 'open' ? '未完成' : filter === 'done' ? '已完成' : filter} ▾
        </button>
      </header>

      <div className="analysis-pills" role="group" aria-label="筛选">
        {(kind === 'events' ? [['all', `全部 ${total}`], ['open', '未完成'], ['done', '已完成']] : REPORT_TYPES).map(([value, label]) => <button
          key={value}
          type="button"
          aria-pressed={filter === value}
          className={filter === value ? 'on' : ''}
          onClick={() => setFilter(value)}
        >{label}</button>)}
      </div>

      <div className="analysis-search">
        <input value={query} spellCheck={false} placeholder={`搜索${copy.title}`} aria-label={`搜索${copy.title}`} onChange={change => setQuery(change.target.value)}/>
      </div>

      <div className="analysis-items">
        {filteredItems.map(item => {
          const status = kind === 'events' ? (item as EventItem).status : (item as ReportItem).status;
          const when = kind === 'events' ? (item as EventItem).last_activity_at : (item as ReportItem).generated_at;
          return <button
            type="button"
            key={item.id}
            className={item.id === selectedId ? 'on' : ''}
            onClick={() => setSelectedId(item.id)}
          >
            <DoneDot done={kind === 'events' ? isDone(status) : true}/>
            <span>
              <strong>{item.title || `${copy.title} #${item.id}`}</strong>
              <small>{statusLabel(status)}{when ? ` · ${shortDate(when)}` : ''}</small>
            </span>
          </button>;
        })}
        {!filteredItems.length && <p className="analysis-list-empty">没有符合筛选条件的{copy.title}</p>}
      </div>
    </aside>

    <main className="analysis-main">
      <div className="analysis-doc-wrap">
        {kind === 'reports' && report && <div className="analysis-actions">
          <button type="button" onClick={exportMarkdown}>导出 Markdown</button>
          <button type="button" className="primary" onClick={() => window.print()}>导出 PDF</button>
        </div>}

        {detailLoading && !detail && <p className="analysis-loading">正在读取详情…</p>}

        {event && <article className="analysis-doc">
          <div className="analysis-status">
            <DoneDot done={isDone(event.status)}/>
            <span>{statusLabel(event.status)} · {date(event.last_activity_at || event.updated_at)}</span>
          </div>
          <h1>{event.title}</h1>
          <div className="analysis-meta">{`事件 #${event.id}${event.updated_at ? ` · 更新于 ${date(event.updated_at)}` : ''}`}</div>
          <p className="analysis-summary">{event.content || '服务端尚未返回事件内容。'}</p>

          {relatedEvents.length > 0 && <section className="analysis-block">
            <h3>关联事件</h3>
            <div className="cloud-event-relations">
              {relatedEvents.map(relation => <button type="button" key={relation.id} className="cloud-event-relation" onClick={() => openRelatedEvent(relation.id)}>
                <span>{relationLabel(relation)}</span><b>{relation.title || `事件 #${relation.id}`}</b>
                {relation.status && <small>{statusLabel(relation.status)}</small>}
              </button>)}
            </div>
          </section>}

          <section className="analysis-timeline">
            <div className="analysis-block-head"><span>时间线</span><span>服务端记录的进展</span></div>
            {eventNotes.length === 0 && <p className="analysis-list-empty">服务端还没有为这个事件记录进展。</p>}
            {eventNotes.map((note, index) => <div className="analysis-step" key={note.id}>
              <time>{shortDate(note.start_at || note.end_at) || '—'}</time>
              <span className="analysis-step-gutter" aria-hidden="true">
                <i/>
                {index < eventNotes.length - 1 && <b/>}
              </span>
              <div className="analysis-step-body">
                <strong>{note.title}</strong>
                <p>{note.content}</p>
              </div>
            </div>)}
          </section>
        </article>}

        {report && <article className="analysis-doc">
          <div className="analysis-status">
            <span className={`analysis-chip ${isDone(report.status) ? 'done' : ''}`}>{statusLabel(report.status)}</span>
            <span>{date(report.generated_at || report.updated_at)}</span>
          </div>
          <h1>{report.title}</h1>
          <div className="analysis-meta">{(report.period_start || report.period_end)
            ? `报告周期 ${date(report.period_start)} 至 ${date(report.period_end)}`
            : '服务端生成的报告'}</div>
          <pre className="analysis-report">{reportContent}</pre>
          {report.sections?.length ? <div className="cloud-report-sections">
            {report.sections.map(section => <section key={section.id}>
              <h3>{section.title}</h3>
              {(section.items ?? []).map(item => <div className="cloud-report-item" key={item.id}>
                <strong>{item.title}</strong>
                <p>{item.summary}</p>
                {(item.time_text || item.contact_text || item.followup_text) && <small>{[item.time_text, item.contact_text, item.followup_text].filter(Boolean).join(' · ')}</small>}
                {item.sources?.length ? <div className="cloud-report-sources">
                  {item.sources.map((source, index) => source.event_id
                    ? <button type="button" key={`${source.event_id}-${index}`} onClick={() => openRelatedEvent(source.event_id!)}>{source.display_text || `查看事件 #${source.event_id}`}</button>
                    : <span key={index}>{source.display_text}</span>)}
                </div> : null}
              </div>)}
            </section>)}
          </div> : null}
        </article>}
      </div>
    </main>
  </div>;
}
