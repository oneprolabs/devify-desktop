import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FeedbackScreenshot, readScreenshot } from './FeedbackScreenshot';
import { feedbackCategories, feedbackStatuses, imageSource, type DiagnosticSnapshot, type FeedbackDraft, type FeedbackReport } from './feedbackTypes';
import { recordDiagnostic } from './desktopInvoke';
import './feedback.css';

export function Feedback({ category = 'other', page = 'app', relatedBatchIds = [], onClose }: { category?: string; page?: string; relatedBatchIds?: number[]; onClose?: () => void }) {
  const [draft, setDraft] = useState<FeedbackDraft | null>(null);
  const [tab, setTab] = useState<'new' | 'history'>('new');
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [closeFailed, setCloseFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<number | null>(null);
  const [hours, setHours] = useState(0);
  const [history, setHistory] = useState<FeedbackReport[]>([]);
  const [historyPage, setHistoryPage] = useState(1);
  const [hasNext, setHasNext] = useState(false);
  const [loading, setLoading] = useState(true);
  const dialog = useRef<HTMLDialogElement>(null);
  const latest = useRef(draft); latest.current = draft;
  const queue = useRef(Promise.resolve());
  const filesBusy = useRef(false);
  const [adding, setAdding] = useState(false);
  const disabled = busy || adding || editing !== null || !!draft?.attempted;
  const save = (value: FeedbackDraft) => {
    const result = queue.current.catch(() => {}).then(() => invoke<void>('feedback_draft_save', { draft: value }));
    queue.current = result;
    return result;
  };
  useEffect(() => {
    let active = true;
    void invoke<FeedbackDraft>('feedback_draft_get').then(value => {
      if (active) setDraft(value.description || value.contact || value.screenshots.length || value.diagnostics || value.attempted ? value : { ...value, category, page, related_batch_ids: relatedBatchIds });
    }).catch(value => { if (active) setError(String(value)); }).finally(() => { if (active) setLoading(false); });
    recordDiagnostic({ module: 'feedback', operation: 'open' });
    return () => { active = false; };
  }, [category, page]);
  useEffect(() => {
    if (!draft || draft.attempted || busy) return;
    const timer = window.setTimeout(() => { void save(draft).catch(value => setError(`草稿未保存：${String(value)}`)); }, 700);
    return () => window.clearTimeout(timer);
  }, [draft, busy]);
  useEffect(() => { if (onClose && dialog.current && !dialog.current.open) dialog.current.showModal(); }, [onClose]);
  const close = async () => {
    if (busy || adding) return;
    if (editing !== null) { setError('请先保存或取消截图编辑。'); return; }
    try { if (latest.current) await save(latest.current); onClose?.(); } catch (value) { setError(String(value)); setCloseFailed(true); }
  };
  const addFiles = async (files: File[]) => {
    if (filesBusy.current || disabled || !draft) return;
    filesBusy.current = true; setAdding(true); setError('');
    try {
      if (draft.screenshots.length + files.length > 3) throw new Error('最多添加 3 张截图。');
      const images = await Promise.all(files.map(readScreenshot));
      setDraft(value => value && { ...value, screenshots: [...value.screenshots, ...images.map(data => ({ data }))] });
    } catch (value) { setError(String(value)); }
    finally { filesBusy.current = false; setAdding(false); }
  };
  const diagnostics = async (enabled: boolean, range = hours) => {
    setError(''); setBusy(true);
    try {
      const snapshot = enabled ? await invoke<DiagnosticSnapshot>('feedback_diagnostics', { hours: range }) : null;
      setDraft(value => value && { ...value, diagnostics: snapshot });
    } catch (value) { setError(String(value)); }
    finally { setBusy(false); }
  };
  const loadHistory = async (pageNumber = 1) => {
    setTab('history'); setBusy(true); setError('');
    try {
      const result = await invoke<{ results: FeedbackReport[]; next: string | null }>('feedback_list', { page: pageNumber });
      setHistory(result.results); setHasNext(!!result.next); setHistoryPage(pageNumber);
    } catch (value) { setError(String(value)); }
    finally { setBusy(false); }
  };
  const submit = async () => {
    if (!draft) return;
    setBusy(true); setError(''); setNotice('');
    try {
      await save(draft);
      const result = await invoke<FeedbackReport>('feedback_submit', { draft });
      recordDiagnostic({ module: 'feedback', operation: 'success' });
      setNotice(`反馈已收到，编号 ${result.id}。处理进度可在「我的反馈」查看。`);
      setDraft(null);
      setDraft(await invoke<FeedbackDraft>('feedback_draft_get'));
    } catch (value) {
      setError(String(value)); recordDiagnostic({ module: 'feedback', operation: 'failure', code: 'COMMAND_FAILED' });
      // Rust persists attempted=true before POST. Reload the exact snapshot for retries.
      await invoke<FeedbackDraft>('feedback_draft_get').then(setDraft).catch(() => {});
    } finally { setBusy(false); }
  };
  const exportBundle = async () => {
    if (!draft) return;
    setBusy(true); setError('');
    try { await save(draft); const path = await invoke<string>('feedback_export', { draft }); setNotice(`诊断包已保存到：${path}`); }
    catch (value) { setError(String(value)); }
    finally { setBusy(false); }
  };
  const newDraft = async () => {
    setBusy(true); setError('');
    try { await queue.current.catch(() => {}); await invoke('feedback_draft_discard'); setDraft(await invoke<FeedbackDraft>('feedback_draft_get')); setNotice('已新建反馈。'); }
    catch (value) { setError(String(value)); }
    finally { setBusy(false); }
  };
  const content = <div className="feedback-content" onPaste={event => {
    const files = Array.from(event.clipboardData.files).filter(file => file.type.startsWith('image/'));
    if (files.length) { event.preventDefault(); void addFiles(files); }
  }}>
    <header className="feedback-header"><div><h2 id="feedback-title">问题反馈</h2><p>描述遇到的问题，帮助我们改进 AIMyChats。</p></div>{onClose && <button type="button" className="feedback-close" disabled={busy || adding} onClick={() => void close()} aria-label="保存并关闭反馈">×</button>}</header>
    <nav className="feedback-tabs" aria-label="反馈导航"><button type="button" disabled={busy || editing !== null} aria-current={tab === 'new' ? 'page' : undefined} onClick={() => setTab('new')}>提交问题</button><button type="button" disabled={busy || editing !== null} aria-current={tab === 'history' ? 'page' : undefined} onClick={() => void loadHistory()}>我的反馈</button></nav>
    {error && <div className="feedback-error" role="alert">{error}{closeFailed && onClose && <button type="button" onClick={onClose}>不保存本次修改并关闭</button>}</div>}{notice && <div className="feedback-notice" role="status">{notice}</div>}
    {loading && <p role="status">正在读取反馈草稿…</p>}
    {!loading && !draft && tab === 'new' && <button type="button" onClick={() => void newDraft()}>重新加载</button>}
    {tab === 'new' && draft && <>
      <div className="feedback-receiver"><span>接收方</span><strong>{draft.receiver || '尚未配置服务端'}</strong><small>{draft.owner_id ? '使用当前登录账号提交' : '登录后可在线提交；也可先保存诊断包'}</small></div>
      {draft.attempted && <p className="feedback-notice">上次提交结果尚未确认。重试将发送原反馈，避免重复建单。需要修改内容时请新建反馈。</p>}
      <p className="feedback-metadata">{draft.environment?.app_version} · {draft.environment?.platform} / {draft.environment?.arch}{draft.related_batch_ids?.length ? ` · 关联批次 ${draft.related_batch_ids.join('、')}` : ''}</p>
      <form onSubmit={event => { event.preventDefault(); void submit(); }}>
        <label>问题类型<select value={draft.category} disabled={disabled} onChange={event => setDraft({ ...draft, category: event.target.value })}>{Object.entries(feedbackCategories).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></label>
        <label>问题描述 <span className="feedback-required">必填</span><textarea required maxLength={5000} rows={5} value={draft.description} disabled={disabled} placeholder="你做了什么操作？出现了什么问题？你期望的结果是什么？" onChange={event => setDraft({ ...draft, description: event.target.value })}/></label>
        <div className="feedback-field-head"><strong>截图 <small>{draft.screenshots.length}/3</small></strong><span>可以裁剪、遮盖敏感内容</span></div>
        <div className="feedback-dropzone" onDragOver={event => event.preventDefault()} onDrop={event => { event.preventDefault(); void addFiles(Array.from(event.dataTransfer.files)); }}>
          <label className="feedback-file-label">{adding ? '正在处理截图…' : '粘贴、拖入截图，或点击选择'}<input aria-label="添加截图" type="file" accept="image/png,image/jpeg" multiple disabled={disabled} onChange={event => { void addFiles(Array.from(event.target.files || [])); event.target.value = ''; }}/></label>
          <small>PNG/JPEG · 最多 3 张 · 自动压缩至每张 3 MB 以内</small>
        </div>
        <div className="feedback-images">{draft.screenshots.map((image, index) => <figure key={index}><button type="button" disabled={disabled} onClick={() => setEditing(index)} aria-label={`编辑截图 ${index + 1}`}><img src={imageSource(image.data)} alt={`截图 ${index + 1}`}/></button><figcaption><button type="button" disabled={disabled} onClick={() => setEditing(index)}>编辑</button><button type="button" disabled={disabled} onClick={() => setDraft({ ...draft, screenshots: draft.screenshots.filter((_, i) => i !== index) })}>删除</button></figcaption></figure>)}</div>
        {editing !== null && draft.screenshots[editing] && <FeedbackScreenshot key={`${editing}-${draft.screenshots[editing].data.length}`} data={draft.screenshots[editing].data} onCancel={() => setEditing(null)} onSave={data => { setDraft({ ...draft, screenshots: draft.screenshots.map((item, index) => index === editing ? { data } : item) }); setEditing(null); }}/ >}
        <div className="feedback-diagnostics"><label className="feedback-checkbox"><input type="checkbox" checked={!!draft.diagnostics} disabled={disabled} onChange={event => void diagnostics(event.target.checked)}/><strong>附带诊断日志</strong></label><p>包含应用版本、系统平台、操作结果与耗时；不采集聊天内容、数据库、密钥或登录凭据。</p>{draft.diagnostics && <><div className="feedback-diagnostic-tools"><select aria-label="日志范围" value={hours} disabled={disabled} onChange={event => { const value = Number(event.target.value); setHours(value); void diagnostics(true, value); }}><option value={0}>最近 30 分钟</option><option value={24}>最近 24 小时</option></select><span>{draft.diagnostics.events.length} 条运行记录</span><button type="button" disabled={disabled} onClick={() => void diagnostics(true)}>刷新日志</button></div><details><summary>预览将提交的诊断内容</summary><pre>{JSON.stringify(draft.diagnostics, null, 2)}</pre></details></>}</div>
        <label>联系方式 <small>选填</small><input type="text" maxLength={200} value={draft.contact} disabled={disabled} placeholder="邮箱或其他方便联系你的方式" onChange={event => setDraft({ ...draft, contact: event.target.value })}/></label>
        <footer className="feedback-actions"><small>草稿自动保存在本机</small>{draft.attempted && <button type="button" disabled={busy} onClick={() => void newDraft()}>新建反馈</button>}<button type="button" disabled={busy || adding || editing !== null} onClick={() => void exportBundle()}>保存诊断包</button><button type="submit" className="primary" disabled={busy || adding || editing !== null || !draft.description.trim() || !draft.owner_id || !draft.receiver}>{busy ? '正在处理…' : draft.attempted ? '重试提交' : '提交反馈'}</button></footer>
      </form>
    </>}
    {tab === 'history' && <section className="feedback-history" aria-label="我的反馈">{busy ? <p role="status">正在读取反馈…</p> : <>{!history.length && !error && <p className="feedback-empty">还没有提交过反馈。</p>}{history.map(item => <article key={item.id}><header><strong>{feedbackCategories[item.category] || item.category}</strong><span className={`feedback-status ${item.status}`}>{feedbackStatuses[item.status] || item.status}</span></header><p>{item.description}</p><small>{new Date(item.created_at).toLocaleString()} · {item.id}</small>{item.reply && <blockquote><strong>处理回复{item.fix_version ? ` · 修复版本 ${item.fix_version}` : ''}</strong><p>{item.reply}</p></blockquote>}</article>)}<div className="feedback-actions"><button type="button" disabled={historyPage === 1} onClick={() => void loadHistory(historyPage - 1)}>上一页</button><span>第 {historyPage} 页</span><button type="button" disabled={!hasNext} onClick={() => void loadHistory(historyPage + 1)}>下一页</button><button type="button" onClick={() => void loadHistory(historyPage)}>刷新</button></div></>}</section>}
  </div>;
  return onClose ? <dialog ref={dialog} className="feedback-dialog" aria-labelledby="feedback-title" onCancel={event => { event.preventDefault(); void close(); }}>{content}</dialog> : content;
}
