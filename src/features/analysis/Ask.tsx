import { t, tf, ctf, dateLocale, sortLocale } from '../../shared/i18n/locale';
import { LoadingPage } from '../../shared/ui/LoadingSkeleton';
import { SidebarResizeHandle } from '../../shared/ui/SidebarResizeHandle';
import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { EvidenceDrawer, type EvidenceTarget } from '../../shared/ui/EvidenceDrawer';
import { questionAnswerError } from './analysisSettingsText';
import { formatAskAnswer } from './askAnswer';
import { renderAgentMarkdown } from '../../shared/content/summary';

type Citation = { kind: string; id: number; title: string; excerpt: string; conversation_id?: number | null; event_id?: number | null; note_id?: number | null };
type Question = { id: number; thread_id?: number | null; question: string; answer: string; citations: Citation[]; status: string; error?: string; created_at?: string; updated_at?: string; scope?: { since?: string | null; until?: string | null } };
type Settings = { devify?: { active_space_id: string | null } };

const suggestions = ['本周客户相关的风险有哪些？', '我现在有哪些事需要亲自处理？', '最近报价和合同有什么进展？', '哪些待办还没有完成？'];
const rangerOptions = ['全部', '近 7 天', '今天'];
const citationKind = (citation: Citation) => citation.kind.toLowerCase();
const dateLabel = (value?: string) => value ? new Date(value).toLocaleString(dateLocale(), { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : t("刚刚");
const dayLabel = (value?: string) => {
  if (!value) return t("刚刚");
  const date = new Date(value);
  const now = new Date();
  const day = date.toLocaleDateString(dateLocale(), { month: '2-digit', day: '2-digit' });
  if (date.toDateString() === now.toDateString()) return tf("今天 {0}", date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' }));
  return `${day} ${date.toLocaleTimeString(dateLocale(), { hour: '2-digit', minute: '2-digit' })}`;
};
const kindLabel = (kind: string) => ({ message: t("原文"), note: t("笔记"), event: t("待办") } as Record<string, string>)[kind] || kind;

function rangeBounds(range: string) {
  const now = new Date();
  if (range === '今天') {
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    const end = new Date(start);
    end.setDate(end.getDate() + 1);
    return { since: start.toISOString(), until: end.toISOString() };
  }
  if (range === '近 7 天') {
    return {
      since: new Date(now.getTime() - 7 * 86400000).toISOString(),
      until: now.toISOString(),
    };
  }
  return {};
}

function threadRootId(item: Question) {
  return item.thread_id || item.id;
}

function inRange(question: Question, range: string): boolean {
  if (range === '全部') return true;
  if (!question.created_at) return true;
  const time = new Date(question.created_at).getTime();
  if (range === '今天') { const start = new Date(); start.setHours(0, 0, 0, 0); return time >= start.getTime(); }
  return time >= Date.now() - 7 * 86400000;
}

export function AskPage({ onOpenEvent }: { onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<string | null>(null);
  const [questions, setQuestions] = useState<Question[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [asking, setAsking] = useState(false);
  const [range, setRange] = useState('全部');
  const [error, setError] = useState('');
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const askLock=useRef(false);

  const load = useCallback(async () => {
    if (!isTauri()) { setLoading(false); return; }
    setLoading(true); setError('');
    try {
      const settings = await invoke<Settings>('app_settings_get');
      const id = settings.devify?.active_space_id ?? null; setSpaceId(id);
      if (!id) { setQuestions([]); return; }
      const list = await invoke<Question[]>('devify_questions', { spaceId: id });
      setQuestions(list ?? []); setSelectedId(list?.[0] ? threadRootId(list[0]) : null);
    } catch (reason) { setError(questionAnswerError(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const threads = useMemo(() => {
    const groups = new Map<number, Question[]>();
    for (const item of questions) {
      const rootId = threadRootId(item);
      const turns = groups.get(rootId) ?? [];
      turns.push(item);
      groups.set(rootId, turns);
    }
    return [...groups.entries()].map(([id, turns]) => {
      turns.sort((left, right) => new Date(left.created_at || 0).getTime() - new Date(right.created_at || 0).getTime());
      return { id, turns, latest: turns[turns.length - 1] };
    }).sort((left, right) => new Date(right.latest.created_at || 0).getTime() - new Date(left.latest.created_at || 0).getTime());
  }, [questions]);
  const threadTurns = useMemo(() => threads.find(item => item.id === selectedId)?.turns ?? [], [threads, selectedId]);
  const visibleThreads = useMemo(() => threads.filter(item => inRange(item.latest, range)), [threads, range]);
  const threadBusy = asking || threadTurns.some(item => item.status === 'queued');

  const polling = useRef(new Set<number>());
  const timedOut = useRef(new Set<number>());
  const queuedIds = threadTurns.filter(item => item.status === 'queued').map(item => item.id).join(',');
  useEffect(() => {
    if (!spaceId || !queuedIds) return;
    const ids = queuedIds.split(',').map(Number);
    let cancelled = false;
    for (const questionId of ids) {
      if (polling.current.has(questionId) || timedOut.current.has(questionId)) continue;
      polling.current.add(questionId);
      void (async () => {
        try {
          for (let attempt = 0; attempt < 90; attempt += 1) {
            await new Promise(resolve => setTimeout(resolve, 2000));
            if (cancelled) return;
            const answer = await invoke<Question>('devify_question_detail', { spaceId, questionId });
            if (cancelled) return;
            setQuestions(current => current.map(item => item.id === answer.id ? answer : item));
            if (answer.status !== 'queued') return;
          }
          if (!cancelled) {
            timedOut.current.add(questionId);
            setError(t("还没有返回回答，请稍后重试。"));
            setQuestions(current => current.map(item => item.id === questionId && item.status === 'queued'
              ? { ...item, status: 'failed', error: t("还没有返回回答，请稍后重试。") }
              : item));
          }
        } catch (reason) {
          if (!cancelled) setError(questionAnswerError(reason));
        } finally {
          polling.current.delete(questionId);
        }
      })();
    }
    return () => { cancelled = true; ids.forEach(id => polling.current.delete(id)); };
  }, [spaceId, queuedIds]);
  useEffect(() => {
    const node = scrollRef.current;
    if (node) node.scrollTop = node.scrollHeight;
  }, [threadTurns.length, pending, queuedIds]);

  const openCitation = (citation: Citation) => {
    const kind = citationKind(citation);
    const eventId = citation.event_id ?? (kind === 'event' ? citation.id : null);
    if (eventId) { setEvidence({ kind: 'event', id: eventId, title: citation.title, excerpt: citation.excerpt }); return; }
    if (kind === 'message') {
      setEvidence({ kind: 'message', id: citation.id, conversation_id: citation.conversation_id ?? null, title: citation.title, excerpt: citation.excerpt });
      return;
    }
    const noteId = citation.note_id ?? (kind === 'note' ? citation.id : null);
    if (noteId) setEvidence({ kind: 'note', id: noteId, title: citation.title, excerpt: citation.excerpt });
  };

  const ask = async (value = draft) => {
    const question = value.trim();
    if (!spaceId || !question || threadBusy || askLock.current) return;
    askLock.current=true;
    const continuing = selectedId;
    setAsking(true); setPending(question); setDraft(''); setError('');
    try {
      const result = await invoke<Question>('devify_question_ask', {
        spaceId,
        question,
        ...rangeBounds(range),
        ...(continuing ? { threadId: continuing } : {}),
      });
      setQuestions(current => [result, ...current.filter(item => item.id !== result.id)]);
      setSelectedId(result.thread_id || continuing || result.id);
    } catch (reason) { setError(questionAnswerError(reason)); }
    finally { askLock.current=false;setPending(''); setAsking(false); }
  };

  const newThread = () => { setSelectedId(null); setDraft(''); setPending(''); setError(''); };
  const startNew = () => { setSelectedId(null); setPending(''); setError(''); };

  const renderTurn = (question: Question, answer: string, citations: Citation[], status: string, created?: string) => {
    const start = question.scope?.since;
    const end = question.scope?.until;
    const answerRange = start && end ? `${dateLabel(start)} – ${dateLabel(end)}` : start ? tf("从 {0} 起", dateLabel(start)) : end ? tf("截至 {0}", dateLabel(end)) : question.scope ? t("全部") : '';
    const events = citations.filter(citation => citationKind(citation) === 'event' || citation.event_id);
    const sources = citations.filter(citation => !(citationKind(citation) === 'event' || citation.event_id));
    return <div className="ask-turn">
      <div className="ask-user"><p>{question.question}</p></div>
      <div className="ask-bot">
        <span className="ask-avatar" aria-hidden="true">a</span>
        <div className="ask-content">
          {status === 'failed'
            ? <p className="ask-answer-text ask-error">{questionAnswerError(question.error)}</p>
            : status === 'queued'
              ? <p className="ask-pending" role="status"><span className="ask-spinner" aria-hidden="true" />{t("正在检索并生成回答…")}</p>
              : answer.trim()
                ? <div className="ask-answer-text">{renderAgentMarkdown(formatAskAnswer(answer))}</div>
                : <p className="ask-answer-text">{t("这次回答没有内容，请重新提问。")}</p>}

          {events.length > 0 && <section className="ask-block">
            <h3>{t("相关待办")}</h3>
            <div className="ask-events">
              {events.map(citation => <button
                type="button"
                key={`e-${citation.id}`}
                className="ask-event"
                onClick={() => void openCitation(citation)}
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                  <circle cx="8" cy="8" r="6.25" fill="#fff" stroke="#a8a8b0" strokeWidth="1.5"/>
                </svg>
                <strong>{citation.title}</strong>
                {citation.excerpt && <span className="ask-event-meta">{citation.excerpt.slice(0, 18)}</span>}
                <span className="ask-event-arrow" aria-hidden="true">→</span>
              </button>)}
            </div>
          </section>}

          {sources.length > 0 && <section className="ask-block">
            <h3>{t("原文依据")}</h3>
            <div className="ask-quotes">
              {sources.map(citation => <button
                type="button"
                key={`s-${citation.kind}-${citation.id}`}
                className="ask-quote"
                onClick={() => void openCitation(citation)}
              >
                <span className="ask-quote-head">
                  <span className="ask-quote-badge">{kindLabel(citationKind(citation)).slice(0, 1)}</span>
                  <span className="ask-quote-title">{citation.title}</span>
                  <span className="ask-quote-kind">{kindLabel(citationKind(citation))}</span>
                </span>
                <span className="ask-quote-text">{citation.excerpt}</span>
              </button>)}
            </div>
          </section>}

          {created && status !== 'queued' && status !== 'failed' && <p className="ask-foot">{t("基于")} {events.length} {t("个待办、")}{sources.length} {t("条原文 ·")} {dayLabel(created)}{answerRange && <> {t("· 范围：")}{answerRange}</>}</p>}
        </div>
      </div>
    </div>;
  };

  if (!isTauri()) return <div className="ask-page ask-page-empty"><h1>{t("问答")}</h1><p>{t("问答基于已上传记录生成，请在桌面应用中使用。")}</p></div>;
  if (loading) return <LoadingPage title={t("问答")} />;

  return <div className="ask-page">
    <aside className="ask-side">
      <h1>{t("问答")}</h1>
      <p className="ask-side-sub">{t("自由提问，跨会话检索")}</p>
      <button type="button" className="ask-new" onClick={newThread}><span aria-hidden="true">＋</span>{t("新对话")}</button>
      <div className="ask-side-label">{t("最近")}</div>
      <div className="ask-threads">
        {visibleThreads.map(item => <button
          type="button"
          key={item.id}
          className={item.id === selectedId ? 'on' : ''}
          onClick={() => { setSelectedId(item.id); setPending(''); }}
        >
          <strong>{item.turns[0].question}</strong>
          <small>{dayLabel(item.latest.created_at)} · {item.latest.status === 'queued' ? t("生成中") : item.latest.status === 'failed' ? t("失败") : tf("{0} 轮", item.turns.length)}</small>
        </button>)}
        {!visibleThreads.length && <p className="ask-threads-empty">{range === '全部' ? t("还没有提问记录。") : t("这个时间范围内没有提问。")}</p>}
      </div>
      <SidebarResizeHandle/>
    </aside>

    <main className="ask-main">
      <header className="ask-head">
        <div className="ask-head-text">
          <h2>{threadTurns[0]?.question || pending || t("新对话")}</h2>
          <p>{t("仅检索已上传的会话与待办 · 回答附原文依据")}</p>
        </div>
        <div className="ask-time">
          <span>{t("时间")}</span>
          <div className="ask-time-seg" role="group" aria-label={t("时间范围")}>
            {rangerOptions.map(option => <button
              key={option}
              type="button"
              aria-pressed={range === option}
              className={range === option ? 'on' : ''}
              onClick={() => setRange(option)}
            >{t(option)}</button>)}
          </div>
        </div>
      </header>

      <div className="ask-scroll" ref={scrollRef}>
        {!spaceId && <div className="ask-welcome"><h3>{t("还没有选择空间")}</h3><p>{t("请先在「设置 / 账户」中登录并选择空间。")}</p></div>}
        {error && <p className="ask-error" role="alert">{t(error)}</p>}

        {spaceId && !threadTurns.length && !pending && <>
          <div className="ask-welcome">
            <h3>{t("问问你的聊天记录")}</h3>
            <p>{t("跨已上传的聊天、待办和笔记提问，每条结论都附原文依据。")}</p>
          </div>
          <div className="ask-suggestions">{suggestions.map(item => <button type="button" key={item} onClick={() => void ask(item)}>{item}</button>)}</div>
        </>}

        {threadTurns.map(turn => <Fragment key={turn.id}>{renderTurn(turn, turn.answer || '', turn.citations ?? [], turn.status, turn.created_at)}</Fragment>)}

        {pending && <div className="ask-turn">
          <div className="ask-user"><p>{pending}</p></div>
          <div className="ask-bot">
            <span className="ask-avatar" aria-hidden="true">a</span>
            <div className="ask-content"><p className="ask-pending" role="status"><span className="ask-spinner" aria-hidden="true" />{t("正在检索待办与原始记录…")}</p></div>
          </div>
        </div>}
      </div>

      <footer className="ask-composer">
        <div className="ask-composer-box">
          <textarea
            rows={2}
            value={draft}
            placeholder={t("问任何关于聊天记录的问题，例如：这周客户有哪些没回复的问题？")}
            onChange={event => setDraft(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void ask(); } }}
          />
          <button
            type="button"
            className={`ask-send${draft.trim() && !threadBusy && spaceId ? ' on' : ''}`}
            aria-label={t("发送")}
            onClick={() => void ask()}
            disabled={!draft.trim() || threadBusy || !spaceId}
          >↑</button>
        </div>
      </footer>
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
