import { useMemo } from 'react';
import { t, tf, dateLocale } from '../../shared/i18n/locale';
import { LoadingSkeleton } from '../../shared/ui/LoadingSkeleton';
import { eventViewState } from './chatPresentation';

export type EventItem = { id: number; title: string; content?: string; status?: string | null; last_activity_at?: string | null; updated_at?: string | null };

const dateLabel = (value?: string | number | null) => value ? new Date(typeof value === 'number' ? value * 1000 : value).toLocaleString(dateLocale(), { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : t("时间未知");
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

/** Presentation only; the workbench owns requests, retries, and evidence navigation. */
export function ChatEventsPane({ active, loading, error, events, sessionCount, query, filter, onRetry, onSchedule, onOpenEvidence }: {
  active: boolean;
  loading: boolean;
  error: string;
  events: EventItem[];
  sessionCount: number;
  query: string;
  filter: 'all' | 'pending';
  onRetry: () => void;
  onSchedule: () => void;
  onOpenEvidence: (event: EventItem) => void;
}) {
  const visibleEvents = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    return events.filter(event => (!keyword || `${event.title} ${event.content ?? ''}`.toLowerCase().includes(keyword)) && (filter !== 'pending' || event.status !== 'done'));
  }, [events, query, filter]);
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

  return <div className={`chat-workbench-pane${active ? '' : ' is-hidden'}`}>
    <div className="chat-workbench-subline">
      <span>{loading ? t("正在同步待办…") : tf("{0} 条待办", events.length)} · {sessionCount} {t("个会话 · 云端整理，可人工修改")}</span>
    </div>
    <div className="chat-workbench-feed">
      {loading && <LoadingSkeleton label={t("正在加载待办")} kind="detail" />}
      {error && <div className="chat-workbench-error" role="alert"><span>{events.length ? t("待办刷新失败，显示的是上次读取结果。") : t("暂时无法读取待办，请检查网络或稍后重试。")}</span> <button type="button" onClick={onRetry}>{t("重试")}</button></div>}
      {eventViewState(loading, error, visibleEvents.length) === 'empty' && <div className="chat-workbench-empty-panel">
        <h2>{t("当前会话暂无待办")}</h2>
        <p>{t("上传当前会话并等待服务端分析后，相关待办会出现在这里。")}</p>
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
            <button type="button" className="chat-event-card" onClick={() => onOpenEvidence(event)}>
              <span className="chat-event-head">
                <StateDot done={done}/>
                <strong>{event.title}</strong>
              </span>
              {event.content && <span className="chat-event-content">{event.content}</span>}
              <span className="chat-event-foot">
                <span>{dateLabel(event.last_activity_at || event.updated_at)}</span>
                <span className="chat-event-link">{t("查看完整待办 →")}</span>
              </span>
            </button>
          </div>;
        })}
      </section>)}
    </div>
  </div>;
}
