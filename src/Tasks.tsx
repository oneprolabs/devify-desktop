import { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Collection } from './Collection';

type UploadScope = { usernames: string[]; since: number | null; until: number | null; base_url: string; message_selection?: Record<string, unknown[]> };
type UploadJob = { id: string; account_id: string; space_id: number; batch_id: number | null; status: string; message: string; created_at: string; updated_at: string; scope?: UploadScope | null; manual_group?: string | null; manual_index?: number; manual_total?: number };
type RetryPreview = { fingerprint: string; message_count: number; sessions: Array<{ username: string; display: string; message_count: number }> };
type UploadStatus = { status: string; message_count: number; duplicate_count: number; error_message: string };
type SettingsWithJobs = { upload_queue?: UploadJob[] };
const statusLabels: Record<string, string> = { queued: '等待中', running: '上传中', processing: '处理中', completed: '已完成', failed: '失败', cancelled: '已取消' };
function statusLabel(status: string): string { return statusLabels[status] ?? status; }
function timeLabel(value: string | number): string {
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? '时间未知' : time.toLocaleString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}

export function Tasks({ onNewUpload, startCreating = false }: { onNewUpload: () => void; startCreating?: boolean }) {
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
  const [retryJobId, setRetryJobId] = useState('');
  const [groupRemote, setGroupRemote] = useState<Record<number, UploadStatus>>({});
  const [groupRemoteError, setGroupRemoteError] = useState(false);
  const [groupLoading, setGroupLoading] = useState(false);
  const [sessionNames, setSessionNames] = useState<Record<string, string>>({});
  useEffect(() => { if (startCreating) setCreating(true); }, [startCreating]);
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
  const groups = useMemo(() => {
    const seen = new Set<string>();
    return jobs.filter(job => {
      if (!job.manual_group) return true;
      if (seen.has(job.manual_group)) return false;
      seen.add(job.manual_group);
      return true;
    });
  }, [jobs]);
  const selected = useMemo(() => groups.find(job => (job.manual_group || job.id) === selectedId) ?? groups[0] ?? null, [groups, selectedId]);
  const batches = useMemo(() => selected?.manual_group ? jobs.filter(job => job.manual_group === selected.manual_group).sort((a, b) => (a.manual_index ?? 0) - (b.manual_index ?? 0)) : [], [jobs, selected]);
  const selectedKey = selected?.manual_group || selected?.id || '';
  const groupStatus = batches.some(job => job.status === 'failed') ? 'failed' : batches.some(job => job.status === 'cancelled') ? 'cancelled' : batches.length && batches.length === (selected?.manual_total || batches.length) && batches.every(job => job.status === 'completed') ? 'completed' : batches.some(job => ['running', 'processing', 'uploading'].includes(job.status)) ? 'processing' : 'queued';
  const groupTotal = batches.reduce((sum, job) => sum + Object.values(job.scope?.message_selection ?? {}).reduce((n, items) => n + items.length, 0), 0);
  const groupDone = batches.filter(job => job.status === 'completed').reduce((sum, job) => sum + Object.values(job.scope?.message_selection ?? {}).reduce((n, items) => n + items.length, 0), 0);
  const groupNames = [...new Set(batches.flatMap(job => job.scope?.usernames ?? []))];
  useEffect(() => {
    setGroupRemote({}); setGroupRemoteError(false); setGroupLoading(false);
    if (!selected?.manual_group || !isTauri()) return;
    setGroupLoading(true);
    let active = true;
    const ids = [...new Set(batches.filter(job => job.status === 'completed' && job.batch_id).map(job => job.batch_id as number))];
    void Promise.allSettled(ids.map(async id => [id, await invoke<UploadStatus>('devify_upload_status', { batchId: id })] as const)).then(results => {
      if (!active) return;
      setGroupRemote(Object.fromEntries(results.filter((r): r is PromiseFulfilledResult<readonly [number, UploadStatus]> => r.status === 'fulfilled').map(r => r.value)));
      setGroupRemoteError(results.some(r => r.status === 'rejected')); setGroupLoading(false);
    });
    void invoke<Array<{ username: string; display: string }>>('wechat_sessions', { accountId: selected.account_id }).then(items => { if (active) setSessionNames(Object.fromEntries(items.map(item => [item.username, item.display]))); }).catch(() => { if (active) setSessionNames({}); });
    return () => { active = false; };
  }, [selectedKey, jobs, refreshKey]);
  useEffect(() => {
    if (!jobs.some(job => job.manual_group && ['queued', 'running', 'processing'].includes(job.status))) return;
    const timer = window.setInterval(() => { void load(); setRefreshKey(value => value + 1); }, 10000);
    return () => window.clearInterval(timer);
  }, [jobs, load]);
  const countsComplete = batches.filter(job => job.status === 'completed').every(job => job.batch_id != null && groupRemote[job.batch_id]);
  const totals = Object.values(groupRemote).reduce((sum, item) => ({ added: sum.added + item.message_count, duplicate: sum.duplicate + item.duplicate_count }), { added: 0, duplicate: 0 });
  useEffect(() => { setConfirmRetry(false); setRetryError(''); setRetryPreview(null); setRetryJobId(''); }, [selectedKey]);
  useEffect(() => {
    setRemote(null); setRemoteError('');
    if (selected?.manual_group || !selected?.batch_id || !isTauri()) return;
    let active = true;
    void invoke<UploadStatus>('devify_upload_status', { batchId: selected.batch_id }).then(result => {
      if (active) setRemote(result);
    }).catch(e => { if (active) setRemoteError(`无法获取云端状态：${String(e)}`); });
    return () => { active = false; };
  }, [selected?.id, selected?.batch_id, refreshKey]);
  const refresh = () => { void load(); setRefreshKey(value => value + 1); };
  const currentStatus = selected?.manual_group ? groupStatus : (remote?.status || selected?.status || '');
  const retryScope = jobs.find(job => job.id === retryJobId)?.scope ?? selected?.scope;
  const prepareRetry = async (target = selected?.id) => {
    if (!target || retrying) return;
    setRetryJobId(target);
    setRetrying(true); setRetryError('');
    try { setRetryPreview(await invoke<RetryPreview>('devify_upload_retry_preview', { jobId: target })); setConfirmRetry(true); }
    catch (e) { setRetryError(String(e)); }
    finally { setRetrying(false); }
  };
  const retry = async () => {
    if (!retryJobId || !retryPreview || retrying) return;
    setRetrying(true); setRetryError('');
    try {
      await invoke('devify_upload_retry', { jobId: retryJobId, fingerprint: retryPreview.fingerprint });
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
      <p className="task-list-label">全部任务 · {groups.length}</p>
      {loading && <p className="task-list-empty">正在读取任务…</p>}
      {!isTauri() && <p className="task-list-empty">请在桌面应用中查看任务。</p>}
      {error && <p className="task-list-empty task-error" role="alert">{error}</p>}
      {!loading && isTauri() && !error && !jobs.length && <p className="task-list-empty">还没有上传任务。</p>}
      <div className="task-items">{groups.map(job => <button key={job.manual_group || job.id} type="button" className={selectedKey === (job.manual_group || job.id) ? 'task-item active' : 'task-item'} onClick={() => { setCreating(false); setSelectedId(job.manual_group || job.id); }} aria-pressed={selectedKey === (job.manual_group || job.id)}>
        <span className="task-item-title">{job.manual_group ? `手动上传 · ${[...new Set(jobs.filter(item => item.manual_group === job.manual_group).flatMap(item => item.scope?.usernames ?? []))].map(name => sessionNames[name] || name).join('、')}` : `同步任务 ${job.batch_id ? `#${job.batch_id}` : ''}`}</span><time>{timeLabel(job.updated_at)}</time><small>{job.manual_group ? (() => { const parts = jobs.filter(item => item.manual_group === job.manual_group); const done = parts.filter(item => item.status === 'completed').length; const state = parts.some(item => item.status === 'failed') ? '失败' : parts.some(item => item.status === 'cancelled') ? '已取消' : done === parts.length && parts.length === (job.manual_total || parts.length) ? '已完成' : '进行中'; return `${state} · ${done}/${parts.length} 批`; })() : `${statusLabel(selected?.id === job.id && remote ? remote.status : job.status)} · ${job.message || '等待处理'}`}</small>
      </button>)}</div>
    </aside>
    {creating ? <Collection onSettings={onNewUpload} onComplete={() => { setCreating(false); void load(); }} /> : <section className="task-detail">
      <header className="task-detail-bar">任务 <span>/</span> {selected ? (selected.manual_group ? '手动上传' : `同步任务${selected.batch_id ? ` #${selected.batch_id}` : ''}`) : '全部任务'}</header>
      {selected ? <div className="task-detail-body">
        <p className="task-kicker">手动上传 · {statusLabel(currentStatus)}</p>
        <h2>{selected.manual_group ? '手动上传' : <>同步任务 {selected.batch_id ? <span>#{selected.batch_id}</span> : null}</>}</h2>
        <p className="task-meta">{timeLabel(selected.created_at || selected.updated_at)} · 微信账号 {selected.account_id || '未知'} · 空间 {selected.space_id}</p>
        {selected.manual_group && <div className="task-group-summary"><p>会话：{groupNames.map(name => sessionNames[name] || name).join('、')}</p><p>已处理 {groupDone} / {groupTotal} 条 · {batches.filter(job => job.status === 'completed').length} / {selected.manual_total || batches.length} 批{batches.some(job => job.status === 'failed') ? ` · ${batches.filter(job => job.status === 'failed').length} 批失败` : ''}</p>{batches.length !== (selected.manual_total || batches.length) && <p className="task-error">部分批次记录缺失，无法确认总进度。</p>}{countsComplete && !groupRemoteError ? <p>新增 {totals.added} 条 · 已存在 {totals.duplicate} 条</p> : <p>{groupLoading ? '正在读取云端统计…' : '云端统计暂未全部获取，请刷新查看。'}</p>}<details><summary>查看各批状态</summary>{batches.map(job => <div className="task-batch" key={job.id}><span>第 {(job.manual_index ?? 0) + 1} / {job.manual_total || batches.length} 批 · {statusLabel(job.status)} · {Object.values(job.scope?.message_selection ?? {}).reduce((n, items) => n + items.length, 0)} 条{job.batch_id ? ` · #${job.batch_id}` : ''}</span>{job.status === 'failed' && <><p className="task-error">{job.message}</p><button type="button" onClick={() => void prepareRetry(job.id)} disabled={retrying}>重试该批</button></>}</div>)}</details>{retryError && !confirmRetry && <p className="task-error" role="alert">{retryError}</p>}</div>}
        {(selected.manual_group ? !!retryJobId && confirmRetry : selected.status === 'failed') && (retryScope ? <div className="task-retry">
          {!confirmRetry ? <button type="button" onClick={() => void prepareRetry()} disabled={retrying}>{retrying ? '正在读取…' : '重试上传'}</button> : <div role="alertdialog" aria-label="确认重试上传">
            <strong>确认重试原任务</strong>
            <p>原会话：{retryPreview?.sessions.map(item => `${item.display}（${item.message_count} 条）`).join('、')}</p>
            <p>本次重试共 {retryPreview?.message_count ?? 0} 条消息。</p>
            <p>时间范围：{retryScope.since == null ? '不限起始' : timeLabel(retryScope.since * 1000)} 至 {retryScope.until == null ? '不限结束' : `${timeLabel(retryScope.until * 1000)}（不含）`}</p>
            <p>服务地址：{retryScope.base_url} · 空间 {selected.space_id}</p>
            <p>{selected.manual_group ? '将重新读取该批原选记录；如果内容发生变化，会停止重试。' : '重试会重新读取该范围的本地记录；新产生或更新的记录也可能包含在内。'}</p>
            <div className="task-retry-actions"><button type="button" onClick={() => setConfirmRetry(false)} disabled={retrying}>取消</button><button type="button" onClick={() => void retry()} disabled={retrying}>{retrying ? '正在重试…' : '确认重试'}</button></div>
          </div>}
          {retryError && <p className="task-error" role="alert">{retryError}</p>}
        </div> : <p className="task-meta">此旧任务没有保存原始上传范围，请重新创建采集任务。</p>)}
        {!selected.manual_group && <div className="task-stages">
          <div className="task-stage"><span className="task-stage-dot">{selected.batch_id ? '✓' : '·'}</span><div><strong>打包与上传</strong><p>{selected.batch_id ? '已提交至云端' : selected.message || statusLabel(selected.status)}</p></div></div>
          {selected.batch_id && <div className="task-stage"><span className="task-stage-dot">{remote?.status === 'completed' ? '✓' : '·'}</span><div><strong>云端处理</strong><p>{remote ? statusLabel(remote.status) : remoteError ? '状态暂不可用' : '正在读取状态…'}</p>{remote && <p>新增 {remote.message_count} 条 · 已存在 {remote.duplicate_count} 条</p>}{remote?.error_message && <p className="task-error">{remote.error_message}</p>}{remoteError && <p className="task-error" role="alert">{remoteError}</p>}</div></div>}
        </div>}
      </div> : <div className="task-detail-empty"><h2>从左侧选择任务</h2><p>上传后，可以在这里查看任务状态。</p></div>}
    </section>}
  </div>;
}
