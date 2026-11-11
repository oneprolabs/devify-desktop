import { SidebarResizeHandle } from './SidebarResizeHandle';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { EvidenceDrawer, type EvidenceTarget } from './EvidenceDrawer';

type TopicEvent = { id: number; title?: string; summary?: string; content?: string; status?: string | null; occurred_at?: string | null; event_id?: number };
type TopicRun = { id: number; status?: string; started_at?: string | null; completed_at?: string | null; created_at?: string | null; matched_count?: number; error?: string | null };
type Topic = {
  id: number;
  title: string;
  instruction: string;
  period: 'day' | 'week' | 'month' | string;
  time_local?: string | null;
  timezone?: string;
  enabled: boolean;
  include_open: boolean;
  last_run_at?: string | null;
  last_error?: string;
  updated_at?: string;
  matched_events?: TopicEvent[];
  events?: TopicEvent[];
  execution_history?: TopicRun[];
  runs?: TopicRun[];
  execution_records?: TopicRun[];
  matched_event_ids?: number[];
};
type Settings = { devify?: { active_space_id: number | null } };

const periods = [{ value: 'day', label: '每天' }, { value: 'week', label: '每周' }, { value: 'month', label: '每月' }];
const periodLabel = (value: string) => periods.find(item => item.value === value)?.label ?? value;
const dateLabel = (value?: string | null) => value ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '尚未执行';
const statusLabel = (value?: string | null) => ({ open: '待我处理', in_progress: '推进中', done: '已完成' } as Record<string, string>)[value || ''] || '已匹配';

function StateDot({ done }: { done: boolean }) {
  return <svg className="topic-dot" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25" fill={done ? '#1a1a19' : 'transparent'} stroke={done ? '#1a1a19' : '#c4a06a'} strokeWidth={1.5}/>
    {done && <path d="M5 8.3l2.1 2.1L11 6.1" stroke="#fff" strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
  </svg>;
}

export function TopicsPage({ onOpenEvent }: { onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [topics, setTopics] = useState<Topic[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [creating, setCreating] = useState(false);
  const [query, setQuery] = useState('');
  const [title, setTitle] = useState('');
  const [instruction, setInstruction] = useState('');
  const [period, setPeriod] = useState('day');
  const [time, setTime] = useState('08:30');
  const [loading, setLoading] = useState(isTauri());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [detailLoading, setDetailLoading] = useState(false);
  const [topicDetail, setTopicDetail] = useState<Topic | null>(null);
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);

  const load = useCallback(async () => {
    if (!isTauri()) { setLoading(false); return; }
    setLoading(true); setError('');
    try {
      const settings = await invoke<Settings>('app_settings_get');
      const id = settings.devify?.active_space_id ?? null;
      setSpaceId(id);
      if (!id) { setTopics([]); return; }
      const list = await invoke<Topic[]>('devify_topics', { spaceId: id });
      setTopics(list ?? []);
      setSelectedId(current => current && list.some(item => item.id === current) ? current : list[0]?.id ?? null);
    } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const selected = useMemo(() => topics.find(item => item.id === selectedId) ?? null, [topics, selectedId]);
  useEffect(() => {
    if (!isTauri() || !spaceId || !selectedId) { setTopicDetail(null); return; }
    let active = true;
    setDetailLoading(true);
    void invoke<Topic>('devify_topic_detail', { spaceId, topicId: selectedId })
      .then(value => { if (active) setTopicDetail(value); })
      .catch(() => { if (active) setTopicDetail(null); })
      .finally(() => { if (active) setDetailLoading(false); });
    return () => { active = false; };
  }, [spaceId, selectedId]);

  const detail = topicDetail ?? selected;
  const matchedEvents = detail?.matched_events ?? detail?.events ?? (detail?.matched_event_ids ?? []).map(id => ({ id, title: `事件 #${id}` } as TopicEvent));
  const openCount = matchedEvents.filter(event => event.status !== 'done').length;
  const runs = detail?.execution_history ?? detail?.runs ?? detail?.execution_records ?? [];

  const visibleTopics = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return topics;
    return topics.filter(topic => `${topic.title} ${topic.instruction}`.toLowerCase().includes(keyword));
  }, [topics, query]);
  const groups = useMemo(() => [
    { label: '运行中', items: visibleTopics.filter(topic => topic.enabled) },
    { label: '已暂停', items: visibleTopics.filter(topic => !topic.enabled) },
  ].filter(group => group.items.length), [visibleTopics]);

  const resetForm = () => { setTitle(''); setInstruction(''); setPeriod('day'); setTime('08:30'); setCreating(false); };
  const create = async () => {
    if (!spaceId || !title.trim() || !instruction.trim() || saving) return;
    setSaving(true); setError('');
    try {
      const topic = await invoke<Topic>('devify_topic_create', { spaceId, title: title.trim(), instruction: instruction.trim(), period, timeLocal: time || null, timezone: Intl.DateTimeFormat().resolvedOptions().timeZone });
      setTopics(current => [topic, ...current]); setTopicDetail(topic); setSelectedId(topic.id); resetForm();
    } catch (reason) { setError(String(reason)); }
    finally { setSaving(false); }
  };
  const toggle = async (topic: Topic) => {
    try {
      const updated = await invoke<Topic>('devify_topic_update', { topicId: topic.id, enabled: !topic.enabled });
      setTopics(current => current.map(item => item.id === topic.id ? updated : item)); setTopicDetail(updated);
    } catch (reason) { setError(String(reason)); }
  };
  const remove = async (topic: Topic) => {
    if (!window.confirm(`删除主题「${topic.title}」？`)) return;
    try {
      await invoke('devify_topic_delete', { topicId: topic.id });
      setTopics(current => current.filter(item => item.id !== topic.id));
      setTopicDetail(null);
      setSelectedId(current => current === topic.id ? null : current);
    } catch (reason) { setError(String(reason)); }
  };

  if (!isTauri()) return <div className="topic-page topic-page-empty"><h1>主题订阅</h1><p>主题订阅由服务端保存和执行，请在桌面应用中使用。</p></div>;
  if (loading) return <div className="topic-page topic-page-empty"><h1>主题订阅</h1><p>正在同步主题订阅…</p></div>;

  return <div className="topic-page">
    <aside className="topic-side">
      <h1>主题订阅</h1>
      <p className="topic-side-count">{topics.length} 个订阅</p>
      <input className="topic-side-search" value={query} onChange={event => setQuery(event.target.value)} placeholder="搜索主题" spellCheck={false}/>
      <button type="button" className={`topic-side-new${creating ? ' on' : ''}`} onClick={() => { setCreating(true); setError(''); }}>＋ 新建主题订阅</button>
      <div className="topic-side-list">
        {groups.map(group => <section key={group.label}>
          <h3>{group.label} · {group.items.length}</h3>
          {group.items.map(topic => <button
            type="button"
            key={topic.id}
            className={!creating && topic.id === selectedId ? 'on' : ''}
            onClick={() => { setCreating(false); setSelectedId(topic.id); }}
          >
            <span className={`topic-side-dot${topic.enabled ? ' on' : ''}`} aria-hidden="true"/>
            <span>
              <strong>{topic.title}</strong>
              <small>{periodLabel(topic.period)} · {topic.last_run_at ? dateLabel(topic.last_run_at) : '尚未执行'}</small>
            </span>
          </button>)}
        </section>)}
        {!groups.length && <p className="topic-side-empty">{query.trim() ? '没有匹配的主题。' : '还没有主题订阅。'}</p>}
      </div>
      <SidebarResizeHandle/>
    </aside>

    <main className="topic-main">
      <header className="topic-top">
        <div className="topic-crumbs">主题订阅 <span>/</span> {creating ? '新建' : (detail?.title ?? '全部')}</div>
        <div className="topic-top-actions">
          {!creating && detail && <>
            <button type="button" onClick={() => void toggle(detail)}>{detail.enabled ? '暂停' : '启用'}</button>
            <button type="button" className="danger" onClick={() => void remove(detail)}>删除</button>
          </>}
        </div>
      </header>

      {!spaceId && <div className="topic-empty"><h2>还没有选择 空间</h2><p>请先在「设置 / 账户」中登录并选择空间。</p></div>}
      {error && <p className="topic-error" role="alert">{error}</p>}

      {spaceId && creating && <div className="topic-create-wrap">
        <header>
          <h1>新建主题订阅</h1>
          <p>用一句话描述你想持续关注的内容。保存后先回溯已有事件，之后按周期执行，每期生成一份主题报告。</p>
        </header>
        <input
          className="topic-name-input"
          value={title}
          maxLength={255}
          placeholder="主题名称，例如：报价与合同"
          onChange={event => setTitle(event.target.value)}
        />
        <div className="topic-def-card">
          <textarea
            value={instruction}
            rows={4}
            maxLength={5000}
            placeholder="例如：所有和报价、合同金额、付款有关的讨论，包含微信群和邮件"
            onChange={event => setInstruction(event.target.value)}
          />
          <div className="topic-def-actions">
            <div className="topic-freq" role="group" aria-label="执行周期">
              {periods.map(item => <button
                key={item.value}
                type="button"
                aria-pressed={period === item.value}
                className={period === item.value ? 'on' : ''}
                onClick={() => setPeriod(item.value)}
              >{item.label}</button>)}
            </div>
            <label className="topic-time">
              执行时间
              <input type="time" value={time} onChange={event => setTime(event.target.value)}/>
            </label>
            <button type="button" className="topic-cancel" onClick={resetForm}>取消</button>
            <button type="button" className="topic-save" disabled={saving || !title.trim() || !instruction.trim()} onClick={() => void create()}>{saving ? '保存中…' : '保存并回溯'}</button>
          </div>
        </div>
      </div>}

      {spaceId && !creating && !topics.length && <div className="topic-empty">
        <h2>还没有主题订阅</h2>
        <p>创建一个主题后，服务端会按周期回溯已生成的事件。</p>
        <button type="button" className="topic-save" onClick={() => setCreating(true)}>创建第一个主题</button>
      </div>}

      {spaceId && !creating && detail && <div className="topic-detail-wrap">
        {detailLoading && <p className="topic-detail-loading">正在同步主题详情…</p>}
        <span className={`topic-state${detail.enabled ? ' on' : ''}`}>{detail.enabled ? '运行中' : '已暂停'}</span>
        <h1>{detail.title}</h1>
        <div className="topic-meta">{periodLabel(detail.period)}{detail.time_local ? ` · ${detail.time_local.slice(0, 5)}` : ''} · 上次执行 {dateLabel(detail.last_run_at)} · 服务端执行</div>
        <div className="topic-instruction">{detail.instruction}</div>
        {detail.last_error && <p className="topic-error">上次执行失败：{detail.last_error}</p>}

        <section className="topic-block">
          <div className="topic-block-head">
            <span>匹配的事件 · {matchedEvents.length}</span>
            <span>未完成 {openCount}</span>
          </div>
          {matchedEvents.length ? <div className="topic-event-list">
            {matchedEvents.map(event => {
              const id = event.event_id || event.id;
              const done = event.status === 'done';
              return <button type="button" className="topic-event" key={id} onClick={() => setEvidence({ kind: 'event', id, title: event.title })}>
                <StateDot done={done}/>
                <strong>{event.title || `事件 #${id}`}</strong>
                <small>{statusLabel(event.status)}{done ? '' : ''}</small>
              </button>;
            })}
          </div> : <p className="topic-block-empty">服务端还没有为这个主题匹配到事件。</p>}
        </section>

        <section className="topic-block">
          <div className="topic-block-head"><span>历次执行</span><span>{runs.length} 次</span></div>
          {runs.length ? <div className="topic-run-list">
            {runs.map((run, index) => <div className="topic-run" key={run.id}>
              <span className="topic-run-mark">✓</span>
              <span className="topic-run-label">第 {runs.length - index} 期 · {dateLabel(run.completed_at || run.started_at || run.created_at)}</span>
              <span className="topic-run-count">{run.matched_count ?? 0} 个事件</span>
            </div>)}
          </div> : <p className="topic-block-empty">服务端还没有执行记录。</p>}
        </section>
      </div>}
    </main>

    {evidence && <EvidenceDrawer
      spaceId={spaceId}
      target={evidence}
      onClose={() => setEvidence(null)}
      onNavigate={setEvidence}
      onOpenDetail={target => { if (target.kind === 'event') onOpenEvent?.(target.id); }}
    />}
  </div>;
}
