import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Question = { id: number; question: string; answer: string; citations: Array<{ kind: string; id: number; title: string; excerpt: string }>; status: string; error?: string; created_at?: string; updated_at?: string };
type Settings = { devify?: { active_space_id: number | null } };
const suggestions = ['本周客户相关的风险有哪些？', '我现在有哪些事需要亲自处理？', '最近报价和合同有什么进展？', '哪些事件还没有完成？'];
const dateLabel = (value?: string) => value ? new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' }) : '刚刚';

export function AskPage() {
  const [spaceId, setSpaceId] = useState<number | null>(null);
  const [questions, setQuestions] = useState<Question[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [draft, setDraft] = useState('');
  const [pending, setPending] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [asking, setAsking] = useState(false);
  const [error, setError] = useState('');

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
  return <div className="ask-page"><aside className="ask-history"><header><h1>问答</h1><p>基于已上传的聊天与服务端事件提问</p></header><button type="button" className="ask-new" onClick={() => { setSelectedId(null); setDraft(''); }}>＋ 新问题</button><div className="ask-history-list">{questions.map(item => <button type="button" key={item.id} className={item.id === selectedId ? 'selected' : ''} onClick={() => setSelectedId(item.id)}><strong>{item.question}</strong><small>{dateLabel(item.created_at)} · {item.status === 'ready' ? '已回答' : item.status}</small></button>)}</div></aside><main className="ask-main"><header className="ask-main-head"><div><h2>{selected?.question || '问问你的聊天记录'}</h2><p>回答基于已上传记录和服务端生成的事件，每条结论附原文依据。</p></div><span>当前空间 · {spaceId ? '已连接' : '未选择'}</span></header><div className="ask-scroll">{!spaceId && <div className="ask-empty"><h3>还没有选择 Devify 空间</h3><p>请先在「设置 / 账户」中登录并选择空间。</p></div>}{spaceId && !selected && !pending && <><div className="ask-welcome"><h3>问问你的聊天记录</h3><p>跨已上传的聊天、事件和笔记提问，答案由 Devify 服务端生成。</p></div><div className="ask-suggestions">{suggestions.map(item => <button type="button" key={item} onClick={() => void ask(item)}>{item}</button>)}</div></>}{pending && <div className="ask-turn"><div className="ask-user-bubble">{pending}</div><div className="ask-pending">服务端正在检索并生成回答…</div></div>}{selected && <div className="ask-turn"><div className="ask-user-bubble">{selected.question}</div>{selected.status === 'failed' ? <div className="ask-answer ask-error">{selected.error || '服务端回答失败。'}</div> : <div className="ask-answer"><p>{selected.answer || '服务端尚未返回回答。'}</p>{selected.citations?.length > 0 && <div className="ask-citations"><strong>原文依据</strong>{selected.citations.map(citation => <div className="ask-citation" key={`${citation.kind}-${citation.id}`}><span>{citation.title}</span><small>{citation.excerpt}</small></div>)}</div>}</div>}</div>}{error && <p className="ask-error" role="alert">{error}</p>}</div><form className="ask-compose" onSubmit={event => { event.preventDefault(); void ask(); }}><textarea value={draft} onChange={event => setDraft(event.target.value)} rows={2} placeholder="输入问题，例如：哪些客户事项还没有跟进？" disabled={!spaceId || asking} /><button type="submit" disabled={!spaceId || asking || !draft.trim()}>{asking ? '生成中…' : '发送 ↑'}</button></form></main></div>;
}
