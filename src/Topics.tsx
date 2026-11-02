import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type TopicEvent = { id: number; title?: string; summary?: string; content?: string; status?: string | null; occurred_at?: string | null; event_id?: number };
type TopicRun = { id: number; status?: string; started_at?: string | null; completed_at?: string | null; created_at?: string | null; matched_count?: number; error?: string | null };
type Topic = {
  id: number;
  title: string;
  instruction: string;
  period: 'day' | 'week' | 'month' | string;
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
const dateLabel = (value?: string | null) => value ? new Date(value).toLocaleString('zh-CN', { dateStyle: 'medium', timeStyle: 'short' }) : '尚未执行';

export function TopicsPage({ onOpenEvent }: { onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [topics, setTopics] = useState<Topic[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [creating, setCreating] = useState(false);
  const [title, setTitle] = useState('');
  const [instruction, setInstruction] = useState('');
  const [period, setPeriod] = useState('week');
  const [loading, setLoading] = useState(isTauri());
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');
  const [detailLoading, setDetailLoading] = useState(false);
  const [topicDetail, setTopicDetail] = useState<Topic | null>(null);

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
  const runs = detail?.execution_history ?? detail?.runs ?? detail?.execution_records ?? [];
  const topicGroups = useMemo(() => [
    { label: '运行中', items: topics.filter(topic => topic.enabled) },
    { label: '已暂停', items: topics.filter(topic => !topic.enabled) },
  ].filter(group => group.items.length), [topics]);
  const resetForm = () => { setTitle(''); setInstruction(''); setPeriod('week'); setCreating(false); };
  const create = async () => {
    if (!spaceId || !title.trim() || !instruction.trim() || saving) return;
    setSaving(true); setError('');
    try {
      const topic = await invoke<Topic>('devify_topic_create', { spaceId, title: title.trim(), instruction: instruction.trim(), period });
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
    try { await invoke('devify_topic_delete', { topicId: topic.id }); setTopics(current => current.filter(item => item.id !== topic.id)); setTopicDetail(null); setSelectedId(current => current === topic.id ? null : current); }
    catch (reason) { setError(String(reason)); }
  };

  if (!isTauri()) return <div className="topic-page topic-empty"><h1>主题</h1><p>主题订阅由 Devify 服务端保存和执行，请在桌面应用中使用。</p></div>;
  if (loading) return <div className="topic-page topic-empty"><h1>主题</h1><p>正在同步主题订阅…</p></div>;
  return <div className="topic-page">
    <header className="topic-head"><div><p className="topic-kicker">DEVIFY · 服务端订阅</p><h1>主题</h1><p>持续关注你关心的内容，周期执行由服务端完成，本地只管理订阅。</p></div><button type="button" className="topic-primary" onClick={() => setCreating(true)} disabled={!spaceId}>＋ 新建主题订阅</button></header>
    {!spaceId && <div className="topic-empty"><h2>还没有选择 Devify 空间</h2><p>请先在「设置 / 账户」中登录并选择空间。</p></div>}
    {error && <p className="topic-error" role="alert">{error}</p>}
    {spaceId && creating && <section className="topic-create"><h2>新建主题订阅</h2><p>用一句话描述你想持续关注的内容。保存后由服务端按周期执行。</p><label>主题名称<input value={title} onChange={event => setTitle(event.target.value)} placeholder="例如：报价与合同" maxLength={255} /></label><label>关注说明<textarea value={instruction} onChange={event => setInstruction(event.target.value)} rows={4} placeholder="例如：所有和报价、合同金额、付款有关的讨论，包含群聊和邮件" maxLength={5000} /></label><div className="topic-create-row"><label>执行周期<select value={period} onChange={event => setPeriod(event.target.value)}>{periods.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}</select></label><span className="topic-create-spacer" /><button type="button" className="topic-secondary" onClick={resetForm}>取消</button><button type="button" className="topic-primary" onClick={() => void create()} disabled={saving || !title.trim() || !instruction.trim()}>{saving ? '保存中…' : '保存订阅'}</button></div></section>}
    {spaceId && !creating && !topics.length && <div className="topic-empty"><h2>还没有主题订阅</h2><p>创建一个主题后，服务端会按周期回溯已生成的事件。</p><button type="button" className="topic-primary" onClick={() => setCreating(true)}>创建第一个主题</button></div>}
    {spaceId && topics.length > 0 && <div className="topic-layout"><aside className="topic-list" aria-label="主题订阅列表">{topicGroups.map(group => <div className="topic-group" key={group.label}><h3>{group.label} · {group.items.length}</h3>{group.items.map(topic => <button type="button" key={topic.id} className={topic.id === selectedId ? 'selected' : ''} onClick={() => setSelectedId(topic.id)}><span className="topic-list-title">{topic.title}</span><small>{periodLabel(topic.period)} · {topic.last_run_at ? dateLabel(topic.last_run_at) : '尚未执行'}</small></button>)}</div>)}</aside><article className="topic-detail">{detail && <>{detailLoading && <p className="topic-detail-loading">正在同步主题详情…</p>}<div className="topic-detail-top"><div><span className={detail.enabled ? 'topic-status on' : 'topic-status'}>{detail.enabled ? '运行中' : '已暂停'}</span><h2>{detail.title}</h2><p>{periodLabel(detail.period)} · 上次执行：{dateLabel(detail.last_run_at)}</p></div><div className="topic-detail-actions"><button type="button" className="topic-secondary" onClick={() => void toggle(detail)}>{detail.enabled ? '暂停订阅' : '启用订阅'}</button><button type="button" className="topic-danger" onClick={() => void remove(detail)}>删除</button></div></div><div className="topic-instruction">{detail.instruction}</div>{detail.last_error && <p className="topic-error">上次执行失败：{detail.last_error}</p>}<section className="topic-detail-section"><div className="topic-section-head"><div><h3>匹配事件</h3><p>按最近一次主题执行匹配到的事件。</p></div><span>{matchedEvents.length} 条</span></div>{matchedEvents.length ? <div className="topic-matched-events">{matchedEvents.map(event => <button type="button" className="topic-matched-event" key={event.id} onClick={() => onOpenEvent?.(event.event_id || event.id)}><div><strong>{event.title || `事件 #${event.event_id || event.id}`}</strong><p>{event.summary || event.content || '服务端未返回摘要。'}</p></div><small>{event.status || '已匹配'}{event.occurred_at ? ` · ${dateLabel(event.occurred_at)}` : ''}</small></button>)}</div> : <p className="topic-detail-empty">尚未匹配到事件。</p>}</section><section className="topic-detail-section"><div className="topic-section-head"><div><h3>执行历史</h3><p>服务端按周期运行的记录。</p></div><span>{runs.length} 次</span></div>{runs.length ? <div className="topic-runs">{runs.map(run => <div className="topic-run" key={run.id}><span className={run.status === 'failed' ? 'topic-run-dot failed' : 'topic-run-dot'} /><div><strong>{run.status === 'failed' ? '执行失败' : run.status === 'running' ? '执行中' : '已完成'}</strong><small>{dateLabel(run.completed_at || run.started_at || run.created_at)}{typeof run.matched_count === 'number' ? ` · 匹配 ${run.matched_count} 条事件` : ''}</small>{run.error && <p>{run.error}</p>}</div></div>)}</div> : <p className="topic-detail-empty">还没有执行记录。</p>}</section><div className="topic-server-note"><strong>服务端执行</strong><p>主题回溯、事件匹配和主题报告由 Devify 服务端生成；桌面端不会在本地伪造结果。下一次执行完成后，相关报告会同步到「报告」。</p></div></>}</article></div>}
  </div>;
}
