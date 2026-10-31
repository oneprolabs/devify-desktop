import { useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Kind = 'events' | 'reports';
type EventItem = { id: number; title: string; content: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };
type ReportSection = { id: number; title: string; position: number; items?: Array<{ id: number; title: string; summary: string; label?: string; time_text?: string; contact_text?: string; followup_text?: string; sources?: Array<{ display_text?: string; event_id?: number | null; note_id?: number | null }> }> };
type ReportItem = { id: number; title: string; body?: string; content?: unknown; status: string; period_start?: string | null; period_end?: string | null; generated_at?: string | null; updated_at?: string | null; sections?: ReportSection[] };
type EventNote = { id: number; title: string; content: string; start_at?: string | null; end_at?: string | null };
type EventDetail = EventItem & { note_ids?: number[]; relation_ids?: number[] };
type Page<T> = { list: T[]; pagination?: { total?: number; page?: number; next?: string | null } };
type AppSettings = { devify?: { active_space_id: number | null } };

const labels: Record<Kind, { title: string; subtitle: string; empty: string }> = {
  events: { title: '事件', subtitle: '服务端从已上传的聊天中整理出的待办、决定和进展。', empty: '当前空间还没有服务端事件。' },
  reports: { title: '报告', subtitle: '服务端生成的日报、周报和阶段报告。', empty: '当前空间还没有服务端报告。' },
};

function date(value?: string | null): string {
  if (!value) return '时间未知';
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleString('zh-CN', { dateStyle: 'medium', timeStyle: 'short' });
}
function statusLabel(value?: string | null): string {
  return ({ open: '进行中', in_progress: '推进中', done: '已完成', queued: '排队中', ready: '已生成', partial: '部分生成', failed: '生成失败' } as Record<string, string>)[value || ''] || value || '未标记';
}

export function CloudAnalysisPage({ kind }: { kind: Kind }) {
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
        setItems(result.list ?? []);
        setTotal(result.pagination?.total ?? result.list?.length ?? 0);
        setSelectedId(result.list?.[0]?.id ?? null);
      })
      .catch(reason => { if (active) setError(String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [kind]);

  const filteredItems = useMemo(() => items.filter(item => {
    const text = `${item.title} ${kind === 'events' ? (item as EventItem).content : ''}`.toLowerCase();
    if (query.trim() && !text.includes(query.trim().toLowerCase())) return false;
    if (filter === 'open') return (item as EventItem).status !== 'done';
    if (filter === 'done') return (item as EventItem).status === 'done';
    if (filter !== 'all' && kind === 'reports') return item.title.includes(filter);
    return true;
  }), [items, kind, query, filter]);
  useEffect(() => { if (selectedId != null && !filteredItems.some(item => item.id === selectedId)) setSelectedId(filteredItems[0]?.id ?? null); }, [filteredItems, selectedId]);
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
  const reportContent = report?.body || (typeof report?.content === 'string' ? report.content : report?.content ? JSON.stringify(report.content, null, 2) : '服务端尚未返回正文。');
  const exportMarkdown = () => {
    if (!report) return;
    const sections = report.sections?.map(section => `## ${section.title}\n\n${(section.items ?? []).map(item => `### ${item.title}\n\n${item.summary}`).join('\n\n')}`).join('\n\n') ?? '';
    const markdown = `# ${report.title}\n\n${reportContent}\n\n${sections}`;
    const link = document.createElement('a'); link.href = URL.createObjectURL(new Blob([markdown], { type: 'text/markdown;charset=utf-8' })); link.download = `${report.title || 'Devify报告'}.md`; link.click(); URL.revokeObjectURL(link.href);
  };

  return <div className="cloud-analysis-page">
    <header className="cloud-analysis-head"><div><p className="cloud-analysis-kicker">DEVIFY · 服务端同步</p><h1>{copy.title}</h1><p>{copy.subtitle}</p></div><span className="cloud-analysis-count">{spaceId ? `${total} 条` : '未选择空间'}</span></header>
    {!isTauri() && <div className="cloud-analysis-empty"><h2>请在桌面应用中查看</h2><p>事件和报告由 Devify 服务端生成，桌面端只负责同步并展示。</p></div>}
    {isTauri() && loading && <div className="cloud-analysis-empty"><h2>正在同步{copy.title}…</h2><p>正在从当前 Devify 空间读取最新结果。</p></div>}
    {isTauri() && !loading && error && <div className="cloud-analysis-empty error" role="alert"><h2>同步失败</h2><p>{error}</p></div>}
    {isTauri() && !loading && !error && !items.length && <div className="cloud-analysis-empty"><h2>{copy.empty}</h2><p>完成聊天采集并等待服务端处理后，结果会出现在这里。</p></div>}
    {isTauri() && !loading && !error && items.length > 0 && <><div className="cloud-analysis-tools"><input value={query} onChange={event => setQuery(event.target.value)} placeholder={`搜索${copy.title}`} /><div className="cloud-analysis-filters">{(kind === 'events' ? [['all','全部'],['open','未完成'],['done','已完成']] : [['all','全部'],['日报','日报'],['周报','周报'],['月报','月报'],['主题','主题']]).map(([value,label]) => <button type="button" key={value} className={filter === value ? 'active' : ''} onClick={() => setFilter(value)}>{label}</button>)}</div></div><div className="cloud-analysis-body"><aside className="cloud-analysis-list" aria-label={`${copy.title}列表`}>{filteredItems.map(item => <button type="button" key={item.id} className={item.id === selectedId ? 'selected' : ''} onClick={() => setSelectedId(item.id)}><strong>{item.title || `${copy.title} #${item.id}`}</strong><span>{statusLabel(kind === 'events' ? (item as EventItem).status : (item as ReportItem).status)}</span><small>{date(kind === 'events' ? (item as EventItem).last_activity_at : (item as ReportItem).generated_at)}</small></button>)}</aside>{!filteredItems.length ? <div className="cloud-analysis-filter-empty">没有符合筛选条件的{copy.title}</div> : <article className="cloud-analysis-detail">{detailLoading && <p className="cloud-analysis-detail-loading">正在读取详情…</p>}{event && <><div className="cloud-analysis-detail-meta"><span>{statusLabel(event.status)}</span><time>{date(event.last_activity_at || event.updated_at)}</time></div><h2>{event.title}</h2><div className="cloud-analysis-content">{event.content || '服务端尚未返回事件内容。'}</div><div className="cloud-analysis-source-panel"><strong>服务端关联</strong><p>{event.note_ids?.length ? `关联 ${event.note_ids.length} 条分析笔记` : '暂无关联笔记'}{event.relation_ids?.length ? ` · ${event.relation_ids.length} 个事件关系` : ''}</p><small>事件内容和关联关系由 Devify 服务端维护。</small>{eventNotes.length > 0 && <div className="cloud-event-timeline"><strong>进展记录</strong>{eventNotes.map(note => <div className="cloud-event-note" key={note.id}><span>{date(note.start_at || note.end_at)}</span><div><b>{note.title}</b><p>{note.content}</p></div></div>)}</div>}</div></>}{report && <><div className="cloud-analysis-detail-meta"><span>{statusLabel(report.status)}</span><time>{date(report.generated_at || report.updated_at)}</time><span className="cloud-analysis-detail-actions"><button type="button" onClick={exportMarkdown}>导出 Markdown</button><button type="button" onClick={() => window.print()}>打印 / PDF</button></span></div><h2>{report.title}</h2>{(report.period_start || report.period_end) && <p className="cloud-analysis-period">报告周期：{date(report.period_start)} 至 {date(report.period_end)}</p>}<pre className="cloud-analysis-content cloud-analysis-report">{reportContent}</pre>{report.sections?.length ? <div className="cloud-report-sections">{report.sections.map(section => <section key={section.id}><h3>{section.title}</h3>{section.items?.map(item => <div className="cloud-report-item" key={item.id}><strong>{item.title}</strong><p>{item.summary}</p>{(item.time_text || item.contact_text || item.followup_text) && <small>{[item.time_text, item.contact_text, item.followup_text].filter(Boolean).join(' · ')}</small>}</div>)}</section>)}</div> : null}</>}</article>}</div></>}
  </div>;
}
