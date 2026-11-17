import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { FeedbackScreenshot, readScreenshot } from './FeedbackScreenshot';
import { feedbackCategories, feedbackCode, feedbackStatuses, imageSource, type DiagnosticSnapshot, type FeedbackDraft, type FeedbackReport } from './feedbackTypes';
import { recordDiagnostic } from './desktopInvoke';
import './feedback.css';

export function Feedback({ category = 'other', page = 'app', relatedBatchIds = [], onClose, pageMode = false }: { category?: string; page?: string; relatedBatchIds?: number[]; onClose?: () => void; pageMode?: boolean }) {
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
  // Shown as a read-only form row, so users see exactly which client/OS/WeChat build the report describes.
  const environment = [
    draft?.environment?.app_version ? `客户端 ${draft.environment.app_version}` : '',
    [draft?.environment?.os_version || draft?.environment?.platform, draft?.environment?.arch].filter(Boolean).join(' / '),
    draft?.environment?.wechat_version ? `微信 ${draft.environment.wechat_version}` : '',
    draft?.related_batch_ids?.length ? `关联批次 ${draft.related_batch_ids.join('、')}` : '',
  ].filter(Boolean).join(' · ');
  const save = (value: FeedbackDraft) => {
    const result = queue.current.catch(() => {}).then(() => invoke<void>('feedback_draft_save', { draft: value }));
    queue.current = result;
    return result;
  };
  useEffect(() => {
    let active = true;
    void invoke<FeedbackDraft>('feedback_draft_get').then(value => {
      if (active) setDraft(value.description || value.screenshots.length || value.diagnostics || value.attempted ? value : { ...value, category, page, related_batch_ids: relatedBatchIds });
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
      await invoke<FeedbackReport>('feedback_submit', { draft });
      recordDiagnostic({ module: 'feedback', operation: 'success' });
      setNotice('反馈已提交。处理进度可在「我的反馈」查看。');
      setDraft(null);
      setDraft(await invoke<FeedbackDraft>('feedback_draft_get'));
    } catch (value) {
      setError(String(value)); recordDiagnostic({ module: 'feedback', operation: 'failure', code: 'COMMAND_FAILED' });
      // Rust persists attempted=true before POST. Reload the exact snapshot for retries.
      await invoke<FeedbackDraft>('feedback_draft_get').then(setDraft).catch(() => {});
    } finally { setBusy(false); }
  };
  const newDraft = async () => {
    setBusy(true); setError('');
    try { await queue.current.catch(() => {}); await invoke('feedback_draft_discard'); setDraft(await invoke<FeedbackDraft>('feedback_draft_get')); setNotice('已新建反馈。'); }
    catch (value) { setError(String(value)); }
    finally { setBusy(false); }
  };
  const content = <div className={`feedback-content${pageMode ? ' feedback-page' : ''}`} onPaste={event => {
    const files = Array.from(event.clipboardData.files).filter(file => file.type.startsWith('image/'));
    if (files.length) { event.preventDefault(); void addFiles(files); }
  }}>
    {!pageMode && <header className="feedback-header"><div><h2 id="feedback-title">问题反馈</h2><p>描述遇到的问题，帮助我们改进 AIMyChats。</p></div>{onClose && <button type="button" className="feedback-close" disabled={busy || adding} onClick={() => void close()} aria-label="保存并关闭反馈">×</button>}</header>}
    <nav className="feedback-tabs" aria-label="反馈导航"><button type="button" disabled={busy || editing !== null} aria-current={tab === 'new' ? 'page' : undefined} onClick={() => { setTab('new'); setError(''); }}>提交问题</button><button type="button" disabled={busy || editing !== null} aria-current={tab === 'history' ? 'page' : undefined} onClick={() => void loadHistory()}>我的反馈</button></nav>
    {error && <div className="feedback-error" role="alert">{error}{tab === 'history' && !busy && <button type="button" onClick={() => void loadHistory(historyPage)}>重试</button>}{closeFailed && onClose && <button type="button" onClick={onClose}>不保存本次修改并关闭</button>}</div>}{notice && <div className="feedback-notice" role="status">{notice}</div>}
    {loading && <p role="status">正在读取反馈草稿…</p>}
    {!loading && !draft && tab === 'new' && <button type="button" onClick={() => void newDraft()}>重新加载</button>}
    {tab === 'new' && draft && <>
      {(!draft.owner_id || !draft.receiver) && <p className="feedback-metadata">{!draft.receiver ? '尚未配置服务端地址，暂时无法提交反馈。' : '登录后可在线提交反馈。'}</p>}
      {draft.attempted && <p className="feedback-notice">上次提交结果尚未确认。重试将发送原反馈，避免重复建单。需要修改内容时请新建反馈。</p>}
      <form onSubmit={event => { event.preventDefault(); void submit(); }}>
        <div className="feedback-environment"><span className="feedback-environment-label">运行环境</span><span className="feedback-environment-value">{environment || '正在读取…'}</span></div>
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
        <footer className="feedback-actions"><small>草稿自动保存在本机</small>{draft.attempted && <button type="button" disabled={busy} onClick={() => void newDraft()}>新建反馈</button>}<button type="submit" className="primary" disabled={busy || adding || editing !== null || !draft.description.trim() || !draft.owner_id || !draft.receiver}>{busy ? '正在处理…' : draft.attempted ? '重试提交' : '提交反馈'}</button></footer>
      </form>
    </>}
    {tab === 'history' && <section className="feedback-history" aria-label="我的反馈">{busy ? <p role="status">正在读取反馈…</p> : <>{!history.length && !error && <p className="feedback-empty">还没有提交过反馈。</p>}{history.map(item => <article key={item.id}><header><strong>{feedbackCategories[item.category] || item.category}</strong><span className={`feedback-status ${item.status}`}>{feedbackStatuses[item.status] || item.status}</span></header><p>{item.description}</p><small>{new Date(item.created_at).toLocaleString()} · {feedbackCode(item.id)}</small>{item.reply && <blockquote><strong>处理回复{item.fix_version ? ` · 修复版本 ${item.fix_version}` : ''}</strong><p>{item.reply}</p></blockquote>}</article>)}{!error && <div className="feedback-actions"><button type="button" disabled={historyPage === 1} onClick={() => void loadHistory(historyPage - 1)}>上一页</button><span>第 {historyPage} 页</span><button type="button" disabled={!hasNext} onClick={() => void loadHistory(historyPage + 1)}>下一页</button><button type="button" onClick={() => void loadHistory(historyPage)}>刷新</button></div>}</>}</section>}
  </div>;
  return onClose ? <dialog ref={dialog} className="feedback-dialog" aria-labelledby="feedback-title" onCancel={event => { event.preventDefault(); void close(); }}>{content}</dialog> : content;
}

/**
 * Settings › 问题反馈: the same submit form and history list as the dialog, laid out as a page
 * so the feedback form is reachable without opening a modal.
 */
export function FeedbackSettings() {
  return <div className="settings-page feedback-settings">
    <header className="page-head">
      <h2>问题反馈</h2>
      <p className="help">描述遇到的问题或改进建议，可附最多 3 张截图与诊断信息。提交后可在「我的反馈」查看处理进度。</p>
    </header>
    <Feedback page="settings" pageMode />
  </div>;
}
