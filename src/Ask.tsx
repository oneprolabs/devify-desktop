import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Citation = { kind: string; id: number; title: string; excerpt: string; conversation_id?: number | null; event_id?: number | null; note_id?: number | null };
type Question = { id: number; question: string; answer: string; citations: Citation[]; status: string; error?: string; created_at?: string; updated_at?: string };
type SourceDetail = { id?: number; title?: string; content?: string; body?: string; text?: string; preview?: string; occurred_at?: string | null; start_at?: string | null; end_at?: string | null; source?: string; conversation_id?: number | string; parts?: Array<{ text?: string | null }> };
type Settings = { devify?: { active_space_id: number | null } };
const suggestions = ['本周客户相关的风险有哪些？', '我现在有哪些事需要亲自处理？', '最近报价和合同有什么进展？', '哪些事件还没有完成？'];
const citationKind = (citation: Citation) => citation.kind.toLowerCase();

const dateLabel = (value?: string) => value ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '刚刚';

export function AskPage({ onOpenEvent }: { onOpenEvent?: (eventId: number) => void }) {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [questions, setQuestions] = useState<Question[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState('');
  const [source, setSource] = useState<SourceDetail | null>(null);
  const [sourceLoading, setSourceLoading] = useState(false);

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
  const openCitation = async (citation: Citation) => {
    const kind = citationKind(citation);
    const eventId = citation.event_id ?? (kind === 'event' ? citation.id : null);
    if (eventId) { onOpenEvent?.(eventId); return; }
    if (!spaceId || !isTauri()) return;
    if (kind === 'message') {
      if (!citation.conversation_id) { setSource({ title: citation.title, content: citation.excerpt }); return; }
      setSourceLoading(true);
      try {
        const message = await invoke<SourceDetail>('devify_message_detail', { spaceId, conversationId: citation.conversation_id, messageId: citation.id });
        setSource({
          ...message,
          title: message.title || citation.title,
          content: message.parts?.map(part => part.text).filter(Boolean).join('\n') || message.preview || citation.excerpt,
        });
      } catch (reason) { setError(String(reason)); }
      finally { setSourceLoading(false); }
      return;
    }
    const noteId = citation.note_id ?? (kind === 'note' ? citation.id : null);
    if (!noteId) return;
    setSourceLoading(true);
    try { setSource(await invoke<SourceDetail>('devify_note_detail', { spaceId, noteId })); }
    catch (reason) { setError(String(reason)); }
    finally { setSourceLoading(false); }
  };

  const ask = async (value = draft) => {
    const question = value.trim();
    if (!spaceId || !question || asking) return;
    setAsking(true); setPending(question); setDraft(''); setError('');
    try { const result = await invoke<Question>('devify_question_ask', { spaceId, question }); setQuestions(current => [result, ...current]); setSelectedId(result.id); }
    catch (reason) { setError(String(reason)); }
    finally { setPending(''); setAsking(false); }
  };

  if (!isTauri()) return <div className="ask-page ask-empty"><h1>问答</h1><p>问答由 Devify 服务端基于已上传记录生成，请在桌面应用中使用。</p></div>;
  if (loading) return <div className="ask-page ask-empty"><h1>问答</h1><p>正在读取问答记录…</p></div>;
  return <div className="ask-page"><aside className="ask-history"><header><h1>问答</h1><p>基于已上传的聊天与服务端事件提问</p></header><button type="button" className="ask-new" onClick={() => { setSelectedId(null); setDraft(''); }}>＋ 新问题</button><div className="ask-history-list">{questions.map(item => <button type="button" key={item.id} className={item.id === selectedId ? 'selected' : ''} onClick={() => setSelectedId(item.id)}><strong>{item.question}</strong><small>{dateLabel(item.created_at)} · {(item.status === 'ready' ? '已回答' : item.status === 'queued' ? '生成中' : item.status === 'failed' ? '生成失败' : item.status)}</small></button>)}</div></aside><main className="ask-main"><header className="ask-main-head"><div><h2>{selected?.question || '问问你的聊天记录'}</h2><p>回答基于已上传记录和服务端生成的事件，每条结论附原文依据。</p></div><span>当前空间 · {spaceId ? '已连接' : '未选择'}</span></header><div className="ask-scroll">{!spaceId && <div className="ask-empty"><h3>还没有选择 Devify 空间</h3><p>请先在「设置 / 账户」中登录并选择空间。</p></div>}{spaceId && !selected && !pending && <><div className="ask-welcome"><h3>问问你的聊天记录</h3><p>跨已上传的聊天、事件和笔记提问，答案由 Devify 服务端生成。</p></div><div className="ask-suggestions">{suggestions.map(item => <button type="button" key={item} onClick={() => void ask(item)}>{item}</button>)}</div></>}{pending && <div className="ask-turn"><div className="ask-user-bubble">{pending}</div><div className="ask-pending">服务端正在检索并生成回答…</div></div>}{selected && <div className="ask-turn"><div className="ask-user-bubble">{selected.question}</div>{selected.status === 'failed' ? <div className="ask-answer ask-error">{selected.error || '服务端回答失败，请重新提问。'}</div> : selected.status === 'queued' ? <div className="ask-answer"><p>服务端正在检索并生成回答…</p></div> : <div className="ask-answer"><p>{selected.answer || '这次回答没有内容，请重新提问。'}</p>{selected.citations?.length > 0 && <div className="ask-citations"><strong>原文依据</strong>{selected.citations.map(citation => <button type="button" className="ask-citation" key={`${citation.kind}-${citation.id}`} onClick={() => void openCitation(citation)}><span>{citation.title}</span><small>{citation.excerpt}</small><em>{citationKind(citation) === 'event' ? '跳转到事件 →' : citationKind(citation) === 'note' ? '查看整理 →' : '查看原文 →'}</em></button>)}</div>}</div>}</div>}{error && <p className="ask-error" role="alert">{error}</p>}</div>{source && <div className="ask-source-backdrop" role="presentation" onMouseDown={event => { if (event.target === event.currentTarget) setSource(null); }}><section className="ask-source-modal" role="dialog" aria-modal="true" aria-labelledby="ask-source-title"><header><div><span>原文</span><h3 id="ask-source-title">{source.title || '原文记录'}</h3></div><button type="button" aria-label="关闭原文" onClick={() => setSource(null)}>×</button></header><div className="ask-source-meta">{source.occurred_at || source.start_at || source.end_at || source.source || source.conversation_id || '来源信息不可用'}</div><p>{source.content || source.body || source.text || '服务端未返回原文内容。'}</p></section></div>}{sourceLoading && <div className="ask-source-loading">正在读取原文…</div>}<form className="ask-compose" onSubmit={event => { event.preventDefault(); void ask(); }}><textarea value={draft} onChange={event => setDraft(event.target.value)} rows={2} placeholder="输入问题，例如：哪些客户事项还没有跟进？" disabled={!spaceId || asking} /><button type="submit" disabled={!spaceId || asking || !draft.trim()}>{asking ? '生成中…' : '发送 ↑'}</button></form></main></div>;
}
