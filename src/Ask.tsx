import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { EvidenceDrawer, type EvidenceTarget } from './EvidenceDrawer';

type Citation = { kind: string; id: number; title: string; excerpt: string; conversation_id?: number | null; event_id?: number | null; note_id?: number | null };
type Question = { id: number; question: string; answer: string; citations: Citation[]; status: string; error?: string; created_at?: string; updated_at?: string };
type Settings = { devify?: { active_space_id: number | null } };

const suggestions = ['本周客户相关的风险有哪些？', '我现在有哪些事需要亲自处理？', '最近报价和合同有什么进展？', '哪些事件还没有完成？'];
const rangerOptions = ['全部', '近 7 天', '今天'];
const citationKind = (citation: Citation) => citation.kind.toLowerCase();
const dateLabel = (value?: string) => value ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '刚刚';
const dayLabel = (value?: string) => {
  if (!value) return '刚刚';
  const date = new Date(value);
  const now = new Date();
  const day = date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' });
  if (date.toDateString() === now.toDateString()) return `今天 ${date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`;
  return `${day} ${date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`;
};
const kindLabel = (kind: string) => ({ message: '原文', note: '笔记', event: '事件' } as Record<string, string>)[kind] || kind;

function inRange(question: Question, range: string): boolean {
  if (range === '全部') return true;
  if (!question.created_at) return true;
  const time = new Date(question.created_at).getTime();
  if (range === '今天') { const start = new Date(); start.setHours(0, 0, 0, 0); return time >= start.getTime(); }
  return time >= Date.now() - 7 * 86400000;
}

export function AskPage({ onOpenEvent }: { onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [questions, setQuestions] = useState<Question[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [asking, setAsking] = useState(false);
  const [range, setRange] = useState('全部');
  const [error, setError] = useState('');
  const [evidence, setEvidence] = useState<EvidenceTarget | null>(null);

  const load = useCallback(async () => {
    if (!isTauri()) { setLoading(false); return; }
    setLoading(true); setError('');
    try {
      const settings = await invoke<Settings>('app_settings_get');
      const id = settings.devify?.active_space_id ?? null; setSpaceId(id);
      if (!id) { setQuestions([]); return; }
      const list = await invoke<Question[]>('devify_questions', { spaceId: id });
      setQuestions(list ?? []); setSelectedId(list?.[0]?.id ?? null);
    } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const selected = useMemo(() => questions.find(item => item.id === selectedId) ?? null, [questions, selectedId]);
  const visibleQuestions = useMemo(() => questions.filter(question => inRange(question, range)), [questions, range]);

  const polling = useRef(new Set<number>());
  const timedOut = useRef(new Set<number>());
  useEffect(() => {
    if (!spaceId || !selected || selected.status !== 'queued') return;
    const questionId = selected.id;
    if (polling.current.has(questionId) || timedOut.current.has(questionId)) return;
    polling.current.add(questionId);
    let cancelled = false;
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
        if (!cancelled) { timedOut.current.add(questionId); setError('服务端还没有返回回答，请稍后重试。'); }
      } catch (reason) {
        if (!cancelled) setError(String(reason));
      } finally {
        polling.current.delete(questionId);
      }
    })();
    return () => { cancelled = true; polling.current.delete(questionId); };
  }, [spaceId, selected?.id, selected?.status]);

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
    if (!spaceId || !question || asking) return;
    setAsking(true); setPending(question); setDraft(''); setError('');
    try {
      const result = await invoke<Question>('devify_question_ask', { spaceId, question });
      setQuestions(current => [result, ...current]); setSelectedId(result.id);
    } catch (reason) { setError(String(reason)); }
    finally { setPending(''); setAsking(false); }
  };

  const newThread = () => { setSelectedId(null); setDraft(''); setPending(''); setError(''); };
  const startNew = () => { setSelectedId(null); setPending(''); setError(''); };

  const renderTurn = (question: Question, answer: string, citations: Citation[], status: string, created?: string) => {
    const events = citations.filter(citation => citationKind(citation) === 'event' || citation.event_id);
    const sources = citations.filter(citation => !(citationKind(citation) === 'event' || citation.event_id));
    const paragraphs = answer.split(/\n{2,}/).map(text => text.trim()).filter(Boolean);
    return <div className="ask-turn">
      <div className="ask-user"><p>{question.question}</p></div>
      <div className="ask-bot">
        <span className="ask-avatar" aria-hidden="true">a</span>
        <div className="ask-content">
          {status === 'failed'
            ? <p className="ask-answer-text ask-error">{question.error || '服务端回答失败，请重新提问。'}</p>
            : status === 'queued'
              ? <p className="ask-pending">服务端正在检索并生成回答…</p>
              : paragraphs.length
                ? paragraphs.map((text, index) => <p className="ask-answer-text" key={index}>{text}</p>)
                : <p className="ask-answer-text">这次回答没有内容，请重新提问。</p>}

          {events.length > 0 && <section className="ask-block">
            <h3>相关事件</h3>
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
            <h3>原文依据</h3>
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

          {created && <p className="ask-foot">基于 {events.length} 个事件、{sources.length} 条原文 · {dayLabel(created)} · 范围：{range}</p>}
        </div>
      </div>
    </div>;
  };

  if (!isTauri()) return <div className="ask-page ask-page-empty"><h1>问答</h1><p>问答由 Devify 服务端基于已上传记录生成，请在桌面应用中使用。</p></div>;
  if (loading) return <div className="ask-page ask-page-empty"><h1>问答</h1><p>正在读取问答记录…</p></div>;

  return <div className="ask-page">
    <aside className="ask-side">
      <h1>问答</h1>
      <p className="ask-side-sub">自由提问，跨会话检索</p>
      <button type="button" className="ask-new" onClick={newThread}><span aria-hidden="true">＋</span>新对话</button>
      <div className="ask-side-label">最近</div>
      <div className="ask-threads">
        {visibleQuestions.map(item => <button
          type="button"
          key={item.id}
          className={item.id === selectedId ? 'on' : ''}
          onClick={() => { setSelectedId(item.id); setPending(''); }}
        >
          <strong>{item.question}</strong>
          <small>{dayLabel(item.created_at)} · {item.status === 'queued' ? '生成中' : item.status === 'failed' ? '失败' : '1 轮'}</small>
        </button>)}
        {!visibleQuestions.length && <p className="ask-threads-empty">{range === '全部' ? '还没有提问记录。' : '这个时间范围内没有提问。'}</p>}
      </div>
    </aside>

    <main className="ask-main">
      <header className="ask-head">
        <div className="ask-head-text">
          <h2>{selected?.question || (pending || '新对话')}</h2>
          <p>仅检索已上传的会话与事件 · 回答附原文依据</p>
        </div>
        <div className="ask-time">
          <span>时间</span>
          <div className="ask-time-seg" role="group" aria-label="时间范围">
            {rangerOptions.map(option => <button
              key={option}
              type="button"
              aria-pressed={range === option}
              className={range === option ? 'on' : ''}
              onClick={() => setRange(option)}
            >{option}</button>)}
          </div>
        </div>
      </header>

      <div className="ask-scroll">
        {!spaceId && <div className="ask-welcome"><h3>还没有选择 Devify 空间</h3><p>请先在「设置 / 账户」中登录并选择空间。</p></div>}
        {error && <p className="ask-error" role="alert">{error}</p>}

        {spaceId && !selected && !pending && <>
          <div className="ask-welcome">
            <h3>问问你的聊天记录</h3>
            <p>跨已上传的聊天、事件和笔记提问，答案由 Devify 服务端生成，每条结论都附原文依据。</p>
          </div>
          <div className="ask-suggestions">{suggestions.map(item => <button type="button" key={item} onClick={() => void ask(item)}>{item}</button>)}</div>
        </>}

        {pending && <div className="ask-turn">
          <div className="ask-user"><p>{pending}</p></div>
          <div className="ask-bot">
            <span className="ask-avatar" aria-hidden="true">a</span>
            <div className="ask-content"><p className="ask-pending">正在检索事件与原始记录…</p></div>
          </div>
        </div>}

        {selected && renderTurn(selected, selected.answer || '', selected.citations ?? [], selected.status, selected.created_at)}
      </div>

      <footer className="ask-composer">
        <div className="ask-composer-box">
          <textarea
            rows={2}
            value={draft}
            placeholder="问任何关于聊天记录的问题，例如：这周客户有哪些没回复的问题？"
            onChange={event => setDraft(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey) { event.preventDefault(); void ask(); } }}
          />
          <button
            type="button"
            className={`ask-send${draft.trim() && !asking && spaceId ? ' on' : ''}`}
            aria-label="发送"
            onClick={() => void ask()}
            disabled={!draft.trim() || asking || !spaceId}
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
