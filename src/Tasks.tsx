import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Collection } from './Collection';

type UploadScope = { usernames: string[]; since: number | null; until: number | null; base_url: string };
type UploadJob = { id: string; account_id: string; space_id: number; batch_id: number | null; status: string; message: string; created_at: string; updated_at: string; scope?: UploadScope | null };
type RetryPreview = { fingerprint: string; message_count: number; sessions: Array<{ username: string; display: string; message_count: number }> };
type UploadStatus = { status: string; message_count: number; duplicate_count: number; error_message: string };
type SettingsWithJobs = { upload_queue?: UploadJob[] };
const statusLabels: Record<string, string> = { queued: '等待中', running: '上传中', processing: '处理中', completed: '已完成', failed: '失败', cancelled: '已取消' };
function statusLabel(status: string): string { return statusLabels[status] ?? status; }
function timeLabel(value: string | number): string {
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? '时间未知' : time.toLocaleString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}

export function Tasks({ onNewUpload }: { onNewUpload: () => void }) {
  const [creating, setCreating] = useState(false);
  const [jobs, setJobs] = useState<UploadJob[]>([]);
  const [selectedId, setSelectedId] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [error, setError] = useState('');
  const [remote, setRemote] = useState<UploadStatus | null>(null);
  const [remoteError, setRemoteError] = useState('');
  const [refreshKey, setRefreshKey] = useState(0);
  const [confirmRetry, setConfirmRetry] = useState(false);
  const [retryPreview, setRetryPreview] = useState<RetryPreview | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState('');
  const load = useCallback(async () => {
    if (!isTauri()) return;
    setLoading(true); setError('');
    try {
      const result = await invoke<SettingsWithJobs>('app_settings_get');
      setJobs([...(result.upload_queue ?? [])].sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || '')));
    } catch (e) { setError(`读取任务失败：${String(e)}`); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const selected = useMemo(() => jobs.find(job => job.id === selectedId) ?? jobs[0] ?? null, [jobs, selectedId]);
  useEffect(() => {
    setRemote(null); setRemoteError(''); setConfirmRetry(false); setRetryError(''); setRetryPreview(null);
    if (!selected?.batch_id || !isTauri()) return;
    let active = true;
    void invoke<UploadStatus>('devify_upload_status', { batchId: selected.batch_id }).then(result => {
      if (active) setRemote(result);
    }).catch(e => { if (active) setRemoteError(`无法获取云端状态：${String(e)}`); });
    return () => { active = false; };
  }, [selected?.id, selected?.batch_id, refreshKey]);
  const refresh = () => { void load(); setRefreshKey(value => value + 1); };
  const currentStatus = remote?.status || selected?.status || '';
  const prepareRetry = async () => {
    if (!selected || retrying) return;
    setRetrying(true); setRetryError('');
    try { setRetryPreview(await invoke<RetryPreview>('devify_upload_retry_preview', { jobId: selected.id })); setConfirmRetry(true); }
    catch (e) { setRetryError(String(e)); }
    finally { setRetrying(false); }
  };
  const retry = async () => {
    if (!selected || !retryPreview || retrying) return;
    setRetrying(true); setRetryError('');
    try {
      await invoke('devify_upload_retry', { jobId: selected.id, fingerprint: retryPreview.fingerprint });
      setConfirmRetry(false);
      await load();
      setRefreshKey(value => value + 1);
    } catch (e) { setRetryError(String(e)); setConfirmRetry(false); setRetryPreview(null); }
    finally { setRetrying(false); }
  };

  return <div className="task-page">
    <aside className="task-list">
      <header className="task-list-head"><h1>任务</h1><p>查看上传与处理记录</p></header>
      <div className="task-list-tools"><button type="button" className="task-new" onClick={() => setCreating(true)}>＋ 新建采集</button><button type="button" className="task-refresh" onClick={refresh} disabled={loading || !isTauri()}>刷新</button></div>
      <p className="task-list-label">全部任务 · {jobs.length}</p>
      {loading && <p className="task-list-empty">正在读取任务…</p>}
      {!isTauri() && <p className="task-list-empty">请在桌面应用中查看任务。</p>}
      {error && <p className="task-list-empty task-error" role="alert">{error}</p>}
      {!loading && isTauri() && !error && !jobs.length && <p className="task-list-empty">还没有上传任务。</p>}
      <div className="task-items">{jobs.map(job => <button key={job.id} type="button" className={selected?.id === job.id ? 'task-item active' : 'task-item'} onClick={() => { setCreating(false); setSelectedId(job.id); }} aria-pressed={selected?.id === job.id}>
        <span className="task-item-title">同步任务 {job.batch_id ? `#${job.batch_id}` : ''}</span><time>{timeLabel(job.updated_at)}</time><small>{statusLabel(selected?.id === job.id && remote ? remote.status : job.status)} · {job.message || '等待处理'}</small>
      </button>)}</div>
    </aside>
    {creating ? <Collection onSettings={onNewUpload} onComplete={() => { setCreating(false); void load(); }} /> : <section className="task-detail">
      <header className="task-detail-bar">任务 <span>/</span> {selected ? `同步任务${selected.batch_id ? ` #${selected.batch_id}` : ''}` : '全部任务'}</header>
      {selected ? <div className="task-detail-body">
        <p className="task-kicker">手动上传 · {statusLabel(currentStatus)}</p>
        <h2>同步任务 {selected.batch_id ? <span>#{selected.batch_id}</span> : null}</h2>
        <p className="task-meta">{timeLabel(selected.created_at || selected.updated_at)} · 微信账号 {selected.account_id || '未知'} · 空间 {selected.space_id}</p>
        {selected.status === 'failed' && (selected.scope ? <div className="task-retry">
          {!confirmRetry ? <button type="button" onClick={() => void prepareRetry()} disabled={retrying}>{retrying ? '正在读取…' : '重试上传'}</button> : <div role="alertdialog" aria-label="确认重试上传">
            <strong>确认重试原任务</strong>
            <p>原会话：{retryPreview?.sessions.map(item => `${item.display}（${item.message_count} 条）`).join('、')}</p>
            <p>本次重试共 {retryPreview?.message_count ?? 0} 条消息。</p>
            <p>时间范围：{selected.scope.since == null ? '不限起始' : timeLabel(selected.scope.since * 1000)} 至 {selected.scope.until == null ? '不限结束' : `${timeLabel(selected.scope.until * 1000)}（不含）`}</p>
            <p>服务地址：{selected.scope.base_url} · 空间 {selected.space_id}</p>
            <p>重试会重新读取该范围的本地记录；新产生或更新的记录也可能包含在内。</p>
            <div className="task-retry-actions"><button type="button" onClick={() => setConfirmRetry(false)} disabled={retrying}>取消</button><button type="button" onClick={() => void retry()} disabled={retrying}>{retrying ? '正在重试…' : '确认重试'}</button></div>
          </div>}
          {retryError && <p className="task-error" role="alert">{retryError}</p>}
        </div> : <p className="task-meta">此旧任务没有保存原始上传范围，请重新创建采集任务。</p>)}
        <div className="task-stages">
          <div className="task-stage"><span className="task-stage-dot">{selected.batch_id ? '✓' : '·'}</span><div><strong>打包与上传</strong><p>{selected.batch_id ? '已提交至云端' : selected.message || statusLabel(selected.status)}</p></div></div>
          {selected.batch_id && <div className="task-stage"><span className="task-stage-dot">{remote?.status === 'completed' ? '✓' : '·'}</span><div><strong>云端处理</strong><p>{remote ? statusLabel(remote.status) : remoteError ? '状态暂不可用' : '正在读取状态…'}</p>{remote && <p>{remote.message_count} 条消息 · {remote.duplicate_count} 条重复</p>}{remote?.error_message && <p className="task-error">{remote.error_message}</p>}{remoteError && <p className="task-error" role="alert">{remoteError}</p>}</div></div>}
        </div>
      </div> : <div className="task-detail-empty"><h2>从左侧选择任务</h2><p>上传后，可以在这里查看任务状态。</p></div>}
    </section>}
  </div>;
}
