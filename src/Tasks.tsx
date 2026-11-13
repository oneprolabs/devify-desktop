import { LoadingSkeleton } from './LoadingSkeleton';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Collection } from './Collection';
import { advanceCloudRebuild, appendCloudPages, analysisLabel, cloudNeedsPolling, cloudRetryCandidate, fetchCloudPages, linkedLocalJobs, localGroupKey, localQueueCounts, visibleLocalJobs, pollPagePlan, rebuildComplete, rebuildNextPages, refreshInterval, requestIsCurrent, retryCandidate, retryIdentityIsCurrent, sameCloudContext, sameCloudLayout, sameRetryJob, settingsScope, startCloudRebuild, triggerLabel, validateCloudPrefixPage, verifiedDevice, type CloudBatch, type CloudPage, type CloudRebuild, type TaskSettings, type UploadJob } from './taskCloud';

type RetryPreview = { fingerprint: string; message_count: number; sessions: Array<{ username: string; display: string; message_count: number }> };
const PAGE_SIZE = 20;
const statusLabels: Record<string, string> = { created: '待上传', queued: '等待中', pending: '等待中', running: '上传中', uploading: '上传中', processing: '处理中', completed: '已完成', submitted: '已提交', failed: '失败', cancelled: '已取消' };
function statusLabel(status: string): string { return statusLabels[status] ?? (status || '未知'); }
function TaskStatus({ value, label }: { value: string; label?: string }) {
  const tone = value === 'completed' ? 'done' : value === 'failed' ? 'failed' : value === 'cancelled' ? 'muted' : 'pending';
  return <span className={`task-status ${tone}`}><i aria-hidden="true">{tone === 'done' ? '✓' : tone === 'failed' ? '×' : '·'}</i>{label || statusLabel(value)}</span>;
}
function timeLabel(value: string | number | null | undefined): string {
  if (value == null || value === '') return '时间未知';
  const time = new Date(value);
  return Number.isNaN(time.getTime()) ? '时间未知' : time.toLocaleString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' });
}
function shortTimeLabel(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', hour12: false });
}
function localGroupStatus(parts: UploadJob[]): string {
  if (parts.some(job => job.status === 'failed')) return 'failed';
  if (parts.some(job => job.status === 'cancelled')) return 'cancelled';
  if (parts.some(job => ['running', 'uploading', 'processing'].includes(job.status))) return 'processing';
  return parts[0]?.status || 'queued';
}
function count(value: unknown): string { return typeof value === 'number' && Number.isFinite(value) ? String(value) : '—'; }
function localSelectionLabel(job: UploadJob): string {
  const selection = job.scope?.message_selection ?? {};
  const names = job.scope?.usernames ?? [];
  if (!names.length || !names.every(name => Array.isArray(selection[name]))) return '按会话范围';
  return `${names.reduce((sum, name) => sum + selection[name].length, 0)} 条已选记录`;
}
async function currentUserId(): Promise<number | null> {
  const user = await invoke<{ id?: unknown }>('devify_current_user');
  return typeof user.id === 'number' && Number.isSafeInteger(user.id) && user.id > 0 ? user.id : null;
}
async function deviceForUser(userId: number | null): Promise<string | null> {
  if (!userId) return null;
  const identity = await invoke<unknown>('devify_upload_identity').catch(() => null);
  return verifiedDevice(identity, userId);
}

export function Tasks({ onNewUpload, startCreating = false }: { onNewUpload: () => void; startCreating?: boolean }) {
  const [creating, setCreating] = useState(false);
  const [settings, setSettings] = useState<TaskSettings | null>(null);
  const [cloud, setCloud] = useState<CloudBatch[]>([]);
  const [pagination, setPagination] = useState<CloudPage['pagination'] | null>(null);
  const [userId, setUserId] = useState<number | null>(null);
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState('');
  const [loading, setLoading] = useState(isTauri());
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState('');
  const [pollError, setPollError] = useState('');
  const [selectedRetry, setSelectedRetry] = useState<UploadJob | null>(null);
  const [retryBatch, setRetryBatch] = useState<CloudBatch | null>(null);
  const [retryPreview, setRetryPreview] = useState<RetryPreview | null>(null);
  const [retrying, setRetrying] = useState(false);
  const [retryError, setRetryError] = useState('');
  const generation = useRef(0);
  const pageRevision = useRef(0);
  const pollBusy = useRef(false);
  const retryPause = useRef(false);
  const pagesRef = useRef<CloudPage[]>([]);
  const rebuildRef = useRef<CloudRebuild | null>(null);
  const rebuildUser = useRef<number | null>(null);
  const moreRef = useRef(false);
  const pollCursor = useRef(0);
  const nextAttempt = useRef(0);
  const [refreshWake, setRefreshWake] = useState(0);
  const consumedWake = useRef(0);
  const [rebuilding, setRebuilding] = useState(false);
  const cloudRef = useRef(cloud);
  cloudRef.current = cloud;
  const scrollAnchor = useRef<{ key: string; offset: number; top: number } | null>(null);
  const captureScroll = () => {
    const container = document.getElementById('task-items');
    if (!container) return;
    const top = container.getBoundingClientRect().top;
    const first = Array.from(container.querySelectorAll<HTMLElement>('[data-task-id]'))
      .find(node => node.getBoundingClientRect().bottom > top);
    scrollAnchor.current = { key: first?.dataset.taskId ?? '', offset: first ? first.getBoundingClientRect().top - top : 0, top: container.scrollTop };
  };
  useLayoutEffect(() => {
    const anchor = scrollAnchor.current;
    const container = document.getElementById('task-items');
    if (!anchor || !container) return;
    const node = Array.from(container.querySelectorAll<HTMLElement>('[data-task-id]')).find(item => item.dataset.taskId === anchor.key);
    container.scrollTop = node ? container.scrollTop + node.getBoundingClientRect().top - container.getBoundingClientRect().top - anchor.offset : anchor.top;
    scrollAnchor.current = null;
  }, [cloud, settings]);
  useEffect(() => { if (startCreating) setCreating(true); }, [startCreating]);
  useEffect(() => () => { generation.current += 1; }, []);

  const load = useCallback(async () => {
    if (!isTauri()) return;
    const request = ++generation.current;
    pageRevision.current += 1; pagesRef.current = []; rebuildRef.current = null; rebuildUser.current = null; moreRef.current = false; retryPause.current = false; pollCursor.current = 0; nextAttempt.current = 0; setRebuilding(false);
    setLoading(true); setLoadingMore(false); setError(''); setPollError(''); setCloud([]); setPagination(null); setSettings(null); setUserId(null); setDeviceId(null);
    setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); setRetryError(''); setRetrying(false);
    let stored: TaskSettings | null = null;
    try {
      stored = await invoke<TaskSettings>('app_settings_get');
      if (request !== generation.current) return;
      const spaceId = stored.devify?.active_space_id;
      if (!spaceId || !stored.devify?.base_url) { setSettings(stored); return; }
      const userBefore = await currentUserId();
      if (!userBefore) throw new Error('无法验证当前云端登录用户');
      const deviceBefore = await deviceForUser(userBefore);
      const page = await invoke<CloudPage>('devify_upload_list', { spaceId, page: 1, pageSize: PAGE_SIZE });
      const deviceAfter = await deviceForUser(userBefore);
      const userAfter = await currentUserId();
      const latest = await invoke<TaskSettings>('app_settings_get');
      if (request !== generation.current) return;
      if (settingsScope(stored) !== settingsScope(latest) || userBefore !== userAfter) {
        setSettings(null); setError('服务、空间或登录用户已变化，将自动重试。'); return;
      }
      validateCloudPrefixPage(page, 1, spaceId);
      pagesRef.current = [page];
      setUserId(userBefore);
      setDeviceId(deviceBefore && deviceBefore === deviceAfter ? deviceBefore : null);
      setSettings(latest); setCloud(page.list); setPagination(page.pagination);
    } catch (e) {
      // An offline cloud request may still leave a usable local queue, but never expose
      // a queue captured before a concurrent server/space change.
      const latest = stored ? await invoke<TaskSettings>('app_settings_get').catch(() => null) : null;
      if (request !== generation.current) return;
      if (stored && latest && settingsScope(stored) === settingsScope(latest)) setSettings(latest);
      else setSettings(null);
      setError(`无法读取云端任务：${String(e)}。将自动重试。`);
    } finally { if (request === generation.current) setLoading(false); }
  }, []);
  useEffect(() => { void load(); }, [load]);

  const alignLoaded = () => {
    if (!settings || (!settings.devify?.active_space_id || !settings.devify?.base_url) || !pagesRef.current.length) { void load(); return; }
    if (retryPause.current) return;
    pageRevision.current += 1; moreRef.current = false;
    rebuildRef.current = startCloudRebuild(pagesRef.current.length);
    rebuildUser.current = null; nextAttempt.current = 0;
    setRebuilding(true); setLoadingMore(false); setError('');
    setPollError('云端列表正在自动对齐，当前显示的是上次读取的快照。');
    setRefreshWake(value => value + 1);
  };
  const loadMore = () => {
    if (!pagination?.next || loading || loadingMore || retryPause.current) return;
    // Supersede any in-flight poll. Check the two boundaries and the new page;
    // if either moved, retain the old prefix until a verified reconstruction.
    pageRevision.current += 1;
    if (rebuildRef.current) rebuildRef.current = startCloudRebuild(pagesRef.current.length + 1);
    else moreRef.current = true;
    rebuildUser.current = null; nextAttempt.current = 0;
    setRebuilding(true); setLoadingMore(true); setError('');
    setPollError('云端列表正在自动对齐，当前显示的是上次读取的快照。');
    setRefreshWake(value => value + 1);
  };

  const jobs = useMemo(() => settings ? visibleLocalJobs(settings, userId, deviceId) : [], [settings, userId, deviceId]);
  const localCounts = useMemo(() => settings ? localQueueCounts(settings, userId, deviceId) : { hidden: 0, unknown: 0 }, [settings, userId, deviceId]);
  const cloudScope = settings ? settingsScope(settings) : '';
  const cloudSpace = settings?.devify?.active_space_id ?? null;
  const cloudActive = cloudNeedsPolling(cloud) || jobs.some(job => ['queued', 'running', 'processing', 'uploading'].includes(job.status));
  useEffect(() => {
    if (!isTauri() || loading || retrying || selectedRetry) return;
    let active = true;
    const request = generation.current;
    const tick = async () => {
      if (pollBusy.current || retryPause.current || Date.now() < nextAttempt.current) return;
      pollBusy.current = true;
      const revision = pageRevision.current;
      const valid = () => active && requestIsCurrent(request, generation.current, revision, pageRevision.current, retryPause.current);
      try {
        const before = await invoke<TaskSettings>('app_settings_get');
        if (!valid()) return;
        if (!cloudScope || settingsScope(before) !== cloudScope) { void load(); return; }
        if (!cloudSpace || !before.devify?.base_url) { captureScroll(); setSettings(before); return; }
        const beforeUser = await currentUserId();
        if (!valid()) return;
        if (!beforeUser) throw new Error('无法验证当前云端登录用户');
        if (userId != null && beforeUser !== userId || rebuildUser.current != null && beforeUser !== rebuildUser.current) { void load(); return; }
        const beforeDevice = await deviceForUser(beforeUser);
        if (!valid()) return;
        if (deviceId && beforeDevice && deviceId !== beforeDevice) { void load(); return; }

        const snapshot = pagesRef.current;
        let rebuildingNow = rebuildRef.current;
        if (!snapshot.length && !rebuildingNow) rebuildingNow = startCloudRebuild(1);
        if (rebuildingNow && rebuildUser.current == null) rebuildUser.current = beforeUser;
        const selectedCloudId = selectedId.startsWith('cloud:') ? Number(selectedId.slice(6)) : cloudRef.current[0]?.id;
        const selectedPage = snapshot.findIndex(page => page.list.some(batch => batch.id === selectedCloudId)) + 1;
        const activePages = snapshot.filter(page => cloudNeedsPolling(page.list)).map(page => page.pagination.page);
        // Page one is checked every time, so newly inserted records are noticed
        // even if no existing task is active.
        const plan = rebuildingNow ? rebuildNextPages(rebuildingNow) : moreRef.current ?
          [...new Set([1, snapshot.length, snapshot.length + 1])] :
          pollPagePlan(snapshot.length, selectedPage, activePages, pollCursor.current++);
        const refreshed = await fetchCloudPages(plan, cloudSpace, page =>
          invoke<CloudPage>('devify_upload_list', { spaceId: cloudSpace, page, pageSize: PAGE_SIZE }));
        const afterDevice = await deviceForUser(beforeUser);
        const afterUser = await currentUserId();
        const latest = await invoke<TaskSettings>('app_settings_get');
        if (!valid()) return;
        if (settingsScope(latest) !== cloudScope || afterUser && afterUser !== beforeUser) { void load(); return; }
        if (!sameCloudContext(cloudScope, beforeUser, settingsScope(latest), afterUser)) throw new Error('无法验证当前云端登录用户');
        setDeviceId(beforeDevice && beforeDevice === afterDevice ? beforeDevice : null);
        captureScroll(); setSettings(latest);
        if (rebuildingNow) {
          try {
            const next = advanceCloudRebuild(rebuildingNow, refreshed, cloudSpace);
            if (rebuildComplete(next)) {
              pagesRef.current = next.verified;
              const items = next.verified.flatMap(page => page.list);
              cloudRef.current = items; setCloud(items);
              setPagination(next.verified[next.verified.length - 1].pagination);
              setUserId(beforeUser); setError(''); setPollError(''); setLoadingMore(false);
              rebuildRef.current = null; rebuildUser.current = null; setRebuilding(false);
            } else {
              rebuildRef.current = next; setRebuilding(true);
              setPollError('云端列表正在自动对齐，当前显示的是上次读取的快照。');
            }
          } catch {
            rebuildRef.current = startCloudRebuild(rebuildingNow.wanted); rebuildUser.current = null; nextAttempt.current = Date.now() + 10000;
            setRebuilding(true);
            setPollError('云端列表正在变化，当前显示的是上次读取的快照；将自动重试对齐。');
          }
        } else if (moreRef.current) {
          moreRef.current = false;
          const extended = appendCloudPages(snapshot, refreshed, cloudSpace);
          if (extended) {
            pagesRef.current = extended;
            const items = extended.flatMap(page => page.list);
            cloudRef.current = items; setCloud(items);
            setPagination(extended[extended.length - 1].pagination);
            setError(''); setPollError(''); setLoadingMore(false); setRebuilding(false);
          } else {
            rebuildRef.current = startCloudRebuild(snapshot.length + 1); rebuildUser.current = null;
            setRebuilding(true);
            setPollError('云端列表正在自动对齐，当前显示的是上次读取的快照。');
          }
        } else {
          let changed = false;
          for (const page of refreshed) {
            try { validateCloudPrefixPage(page, page.pagination.page, cloudSpace); }
            catch { changed = true; break; }
            if (!sameCloudLayout(snapshot[page.pagination.page - 1], page)) { changed = true; break; }
          }
          if (changed) {
            rebuildRef.current = startCloudRebuild(snapshot.length); rebuildUser.current = null;
            setRebuilding(true);
            setPollError('云端列表正在自动对齐，当前显示的是上次读取的快照。');
          } else {
            const byPage = new Map(refreshed.map(page => [page.pagination.page, page]));
            const updated = snapshot.map(page => byPage.get(page.pagination.page) ?? page);
            pagesRef.current = updated;
            const items = updated.flatMap(page => page.list);
            cloudRef.current = items; setCloud(items);
            setPagination(updated[updated.length - 1].pagination);
            setError(''); setPollError('');
          }
        }
      } catch (e) {
        const latest = await invoke<TaskSettings>('app_settings_get').catch(() => null);
        const latestUser = await currentUserId().catch(() => null);
        if (!valid()) return;
        if (latest && cloudScope && settingsScope(latest) !== cloudScope || latestUser != null && userId != null && latestUser !== userId) { void load(); return; }
        setDeviceId(null);
        if (latest && cloudScope && settingsScope(latest) === cloudScope) { captureScroll(); setSettings(latest); }
        // A failed or invalid page never removes old rows. Retry at the normal
        // bounded cadence, starting the rebuild over to avoid mixed scans.
        if (rebuildRef.current || pagesRef.current.length) {
          rebuildRef.current = startCloudRebuild(rebuildRef.current?.wanted ?? (pagesRef.current.length + (moreRef.current ? 1 : 0)));
          moreRef.current = false;
          rebuildUser.current = null; setRebuilding(true);
        }
        nextAttempt.current = Date.now() + 30000;
        setError('');
        setPollError(`云端自动刷新暂不可用：${String(e)}。当前显示的是上次读取的快照，将自动重试。`);
      } finally { pollBusy.current = false; }
    };
    // A user-requested load-more starts promptly, but timer restarts caused by
    // normal state changes never create an unbounded immediate request loop.
    if (refreshWake !== consumedWake.current) { consumedWake.current = refreshWake; void tick(); }
    const timer = window.setInterval(() => void tick(), refreshInterval(cloudActive, rebuilding));
    return () => { active = false; window.clearInterval(timer); };
  }, [cloudScope, cloudSpace, cloudActive, loading, userId, deviceId, retrying, selectedRetry, selectedId, rebuilding, refreshWake, load]);
  const groups = useMemo(() => {
    const seen = new Set<string>();
    return jobs.filter(job => { const key = localGroupKey(job); if (seen.has(key)) return false; seen.add(key); return true; });
  }, [jobs]);
  const selectedCloud = cloud.find(batch => `cloud:${batch.id}` === selectedId) ?? null;
  const selectedLocal = groups.find(job => `local:${localGroupKey(job)}` === selectedId) ?? null;
  const fallbackCloud = cloud[0] ?? null;
  const fallbackLocal = groups[0] ?? null;
  const activeCloud = selectedCloud || (!selectedLocal ? fallbackCloud : null);
  const activeLocal = selectedLocal || (!activeCloud ? fallbackLocal : null);
  const activeKey = activeCloud ? `cloud:${activeCloud.id}` : activeLocal ? `local:${localGroupKey(activeLocal)}` : '';
  const activeKeyRef = useRef(activeKey);
  activeKeyRef.current = activeKey;
  const parts = activeLocal ? jobs.filter(job => localGroupKey(job) === localGroupKey(activeLocal)).sort((a, b) => (a.manual_index ?? 0) - (b.manual_index ?? 0)) : [];
  const linked = activeCloud && settings ? linkedLocalJobs(activeCloud, settings, deviceId) : [];
  useEffect(() => { retryPause.current = false; setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); setRetryError(''); }, [activeKey]);

  const verifyRetry = async (target: UploadJob, batch: CloudBatch | null, request: number): Promise<UploadJob> => {
    if (!settings) throw new Error('当前设置不可用，请刷新。');
    // Both local and linked retries must still belong to the user and device shown.
    if (!userId || !deviceId || batch && batch.uploaded_by_id !== userId ||
        !await retryIdentityIsCurrent(userId, deviceId, currentUserId, deviceForUser)) {
      setDeviceId(null); throw new Error('无法确认当前云端用户或设备，请刷新。');
    }
    if (request !== generation.current) throw new Error('任务已刷新。');
    if (batch) {
      const status = await invoke<{ batch_id: number; status: string }>('devify_upload_status', { batchId: batch.id });
      if (status.batch_id !== batch.id || status.status !== 'failed') throw new Error('云端状态已变化，请刷新。');
    }
    const latest = await invoke<TaskSettings>('app_settings_get');
    const identityCurrent = await retryIdentityIsCurrent(userId, deviceId, currentUserId, deviceForUser);
    if (request !== generation.current) throw new Error('任务已刷新，请重新选择。');
    if (settingsScope(latest) !== settingsScope(settings)) { void load(); throw new Error('服务或空间已变化。'); }
    if (!identityCurrent) { setDeviceId(null); throw new Error('当前云端用户或设备已变化，请刷新。'); }
    const job = batch ? cloudRetryCandidate(latest, batch, userId, deviceId, target.id) : retryCandidate(latest, target.id, userId, deviceId);
    if (!job || !sameRetryJob(target, job)) throw new Error('任务或重试范围已变化，请刷新。');
    return job;
  };
  const prepareRetry = async (target: UploadJob, batch: CloudBatch | null = null) => {
    if (retrying || !settings) return;
    const request = generation.current;
    const key = batch ? `cloud:${batch.id}` : `local:${localGroupKey(target)}`;
    pageRevision.current += 1; retryPause.current = true;
    if (rebuildRef.current) { rebuildRef.current = startCloudRebuild(rebuildRef.current.wanted); rebuildUser.current = null; }
    let waitingConfirmation = false;
    setRetrying(true); setRetryError(''); setRetryPreview(null); setSelectedRetry(null); setRetryBatch(null);
    try {
      const job = await verifyRetry(target, batch, request);
      if (request !== generation.current || activeKeyRef.current !== key) return;
      const preview = await invoke<RetryPreview>('devify_upload_retry_preview', { jobId: target.id });
      const currentJob = await verifyRetry(job, batch, request);
      if (request !== generation.current || activeKeyRef.current !== key) return;
      waitingConfirmation = true; setSelectedRetry(currentJob); setRetryBatch(batch); setRetryPreview(preview);
    } catch (e) {
      const latest = await invoke<TaskSettings>('app_settings_get').catch(() => null);
      const latestUser = await currentUserId().catch(() => null);
      const latestDevice = await deviceForUser(latestUser);
      if (request !== generation.current) return;
      if (!latest || settingsScope(latest) !== settingsScope(settings) || latestUser !== userId) { void load(); return; }
      if (!latestDevice || latestDevice !== deviceId) setDeviceId(null);
      if (activeKeyRef.current === key) setRetryError(String(e));
    } finally { if (request === generation.current) { retryPause.current = waitingConfirmation; setRetrying(false); } }
  };
  const retry = async () => {
    if (!settings || !selectedRetry || !retryPreview || retrying) return;
    const request = generation.current;
    const key = retryBatch ? `cloud:${retryBatch.id}` : `local:${localGroupKey(selectedRetry)}`;
    setRetrying(true); setRetryError('');
    try {
      await verifyRetry(selectedRetry, retryBatch, request);
      if (request !== generation.current || activeKeyRef.current !== key) return;
      await invoke('devify_upload_retry', { jobId: selectedRetry.id, fingerprint: retryPreview.fingerprint });
      if (request === generation.current) { retryPause.current = false; setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); alignLoaded(); }
    } catch (e) {
      const latest = await invoke<TaskSettings>('app_settings_get').catch(() => null);
      const latestUser = await currentUserId().catch(() => null);
      const latestDevice = await deviceForUser(latestUser);
      if (request !== generation.current) return;
      if (!latest || settingsScope(latest) !== settingsScope(settings) || latestUser !== userId) { void load(); return; }
      if (!latestDevice || latestDevice !== deviceId) setDeviceId(null);
      if (activeKeyRef.current === key) { setRetryError(String(e)); setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); }
    } finally { if (request === generation.current) { retryPause.current = false; setRetrying(false); } }
  };
  const select = (id: string) => { setCreating(false); setSelectedId(id); };
  const localTitle = (job: UploadJob) => job.manual_group ? '手动上传 · 本机未关联批次' : '本机上传任务 · 未关联批次';
  const retryDialog = selectedRetry && retryPreview && selectedRetry.scope && <div className="task-retry"><div role="alertdialog" aria-label="确认重试上传"><strong>确认重试原任务</strong><p>原会话：{retryPreview.sessions.map(item => `${item.display}（${item.message_count} 条）`).join('、')}</p><p>本次重试共 {retryPreview.message_count} 条消息。</p><p>时间范围：{selectedRetry.scope.since == null ? '不限起始' : timeLabel(selectedRetry.scope.since * 1000)} 至 {selectedRetry.scope.until == null ? '不限结束' : `${timeLabel(selectedRetry.scope.until * 1000)}（不含）`}</p><p>服务地址：{selectedRetry.scope.base_url} · 目标空间 {selectedRetry.space_id} · 云端用户 ID {userId} · 微信账号 {selectedRetry.account_id}</p><p>请确认以上云端用户及目标空间。{retryBatch ? '若原内容或附件发生变化将停止重试，请新建采集。' : selectedRetry.manual_group ? '将重新读取该批原选记录；如果内容发生变化，会停止重试。' : '重试会重新读取该范围的本地记录；新产生或更新的记录也可能包含在内。'}</p><div className="task-retry-actions"><button type="button" onClick={() => { retryPause.current = false; setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); setRefreshWake(value => value + 1); }} disabled={retrying}>取消</button><button type="button" onClick={() => void retry()} disabled={retrying}>{retrying ? '正在重试…' : '确认重试'}</button></div></div></div>;

  return <div className="task-page">
    <aside className="task-list">
      <header className="task-list-head"><div><h1>任务</h1><p>当前空间的云端记录与本机队列</p></div></header>
      <div className="task-list-tools"><button type="button" className="task-new" onClick={() => { retryPause.current = false; setSelectedRetry(null); setRetryBatch(null); setRetryPreview(null); setCreating(true); }} disabled={retrying}><span aria-hidden="true">＋</span>新建采集</button></div>
      <div className="task-list-label"><span>云端任务{pagination ? ` · 共 ${pagination.total} 条` : ''}</span><button type="button" className="task-refresh" onClick={alignLoaded} disabled={loading || retrying || !!selectedRetry || !isTauri()} aria-label="刷新任务" title="刷新任务">↻</button></div>
      {loading && <LoadingSkeleton label="正在加载任务" />}
      {!isTauri() && <p className="task-list-empty">请在桌面应用中查看任务。</p>}
      {!loading && settings && !settings.devify?.active_space_id && <p className="task-list-empty">请先选择上传空间。</p>}
      {!loading && settings?.devify?.active_space_id && !settings.devify.base_url && <p className="task-list-empty">请先设置服务地址。</p>}
      {error && <p className="task-list-empty task-error" role="alert">{error}</p>}
      {pollError && <p className={pollError.startsWith('云端自动刷新暂不可用：') ? 'task-list-empty task-error' : 'task-list-empty'} role={pollError.startsWith('云端自动刷新暂不可用：') ? 'alert' : 'status'}>{pollError}</p>}
      <div id="task-items" className="task-items" tabIndex={-1}>
        {!loading && !error && pagination && cloud.length === 0 && <p className="task-list-empty">{pagination.total === 0 ? '此空间暂无云端上传记录。' : '已加载页面没有可显示的记录。'}</p>}
        {cloud.map(batch => <button key={batch.id} data-task-id={`cloud:${batch.id}`} type="button" className={activeKey === `cloud:${batch.id}` ? 'task-item active' : 'task-item'} onClick={() => select(`cloud:${batch.id}`)} aria-pressed={activeKey === `cloud:${batch.id}`}>
          <span className="task-item-marker"><TaskStatus value={batch.status}/></span><span className="task-item-body"><span className="task-item-heading"><strong className="task-item-title">云端批次 #{batch.id}</strong><time dateTime={batch.created_at} title={timeLabel(batch.created_at)}>{shortTimeLabel(batch.created_at)}</time></span><small className="task-item-description">{statusLabel(batch.status)} · 新增 {count(batch.message_count)} 条</small></span>
        </button>)}
        {pagination && <p className="task-list-empty">已加载 {cloud.length} / {pagination.total} 条云端记录。</p>}
        {pagination?.next && <div className="task-list-tools"><button type="button" onClick={() => void loadMore()} disabled={loadingMore}>{loadingMore ? '正在加载…' : '加载更多'}</button></div>}
        {localCounts.unknown > 0 && <p className="task-list-empty" role="status">本机还有 {localCounts.unknown} 条来源或账号归属无法确认的未关联任务。云端是否已创建批次可能无法确认，请先查看云端记录。账号归属无法确认，请重新创建采集；为保护隐私，不显示详情或提供重试。</p>}
        {localCounts.hidden > localCounts.unknown && <p className="task-list-empty" role="status">本机还有 {localCounts.hidden - localCounts.unknown} 条未关联任务，当前无法确认属于此用户、服务、空间和设备，详情未显示。</p>}
        {settings?.devify?.active_space_id && <><div className="task-list-label"><strong>本机队列 · 未关联云端批次</strong></div><p className="task-list-empty">仅显示已确认属于当前云端用户、服务、空间、微信账号和设备的本机任务。</p>
          {groups.length === 0 && <p className="task-list-empty">没有可确认来源的本机未关联批次任务。</p>}
          {groups.map(job => { const key = `local:${localGroupKey(job)}`; const items = jobs.filter(item => localGroupKey(item) === localGroupKey(job)); return <button key={key} data-task-id={key} type="button" className={activeKey === key ? 'task-item active' : 'task-item'} onClick={() => select(key)} aria-pressed={activeKey === key}>
            <span className="task-item-marker"><TaskStatus value={localGroupStatus(items)}/></span><span className="task-item-body"><span className="task-item-heading"><strong className="task-item-title">{localTitle(job)}</strong><time dateTime={job.updated_at} title={timeLabel(job.updated_at)}>{shortTimeLabel(job.updated_at)}</time></span><small className="task-item-description">本机 · {statusLabel(localGroupStatus(items))}{job.manual_group ? ` · ${items.length} 个未关联批次` : ''}</small></span>
          </button>; })}
        </>}
      </div>
      <footer className="task-list-footer"><span>{pagination ? `云端已加载 ${cloud.length} 条` : '云端尚未读取'}</span></footer>
    </aside>
    {creating ? <Collection onSettings={onNewUpload} onComplete={() => { setCreating(false); alignLoaded(); }} /> : <section className="task-detail">
      <header className="task-detail-bar">任务 <span>/</span> {activeCloud ? `云端批次 #${activeCloud.id}` : activeLocal ? localTitle(activeLocal) : '全部任务'}</header>
      {activeCloud ? <div className="task-detail-body">
        <header className="task-overview"><div className="task-overview-copy"><p className="task-kicker">云端上传记录<span aria-hidden="true">·</span><TaskStatus value={activeCloud.status}/></p><h2>云端批次 <span>#{activeCloud.id}</span></h2><p className="task-meta"><time dateTime={activeCloud.created_at}>{timeLabel(activeCloud.created_at)}</time><span>空间 {activeCloud.space_id}</span></p><p className="task-account">微信账号 <span>{activeCloud.source_account_id || '未知'}</span></p><p className="task-meta">平台 {activeCloud.platform || '未知'} · 服务端标记：{triggerLabel(activeCloud.trigger)}</p><p className="task-meta">历史客户端未提交触发方式，旧记录的服务端标记可能不准确。</p></div><TaskStatus value={activeCloud.status}/></header>
        <div className="task-stages"><h3>执行流程</h3>
          <div className={`task-stage ${activeCloud.archive_received_at ? 'is-complete' : ''}`}><span className="task-stage-dot" aria-hidden="true">{activeCloud.archive_received_at ? '✓' : '·'}</span><div><strong>上传归档</strong><p>{activeCloud.archive_received_at ? `服务端接收时间：${timeLabel(activeCloud.archive_received_at)}` : '服务端未提供归档接收时间'}</p></div></div>
          <div className={`task-stage ${activeCloud.status === 'completed' ? 'is-complete' : ''}`}><span className="task-stage-dot" aria-hidden="true">{activeCloud.status === 'completed' ? '✓' : '·'}</span><div><strong>云端解析</strong><p>{statusLabel(activeCloud.status)}{activeCloud.parse_started_at ? ` · 开始于 ${timeLabel(activeCloud.parse_started_at)}` : ''}{activeCloud.completed_at ? ` · 结束于 ${timeLabel(activeCloud.completed_at)}` : ''}</p>{activeCloud.error_message && <p className="task-error">{activeCloud.error_message}</p>}</div><TaskStatus value={activeCloud.status}/></div>
          <div className="task-stage"><span className="task-stage-dot" aria-hidden="true">·</span><div><strong>关联分析</strong><p>{analysisLabel(activeCloud.analysis_summary)}</p>{activeCloud.analysis_summary && typeof activeCloud.analysis_summary.job_count === 'number' && activeCloud.analysis_summary.job_count > 0 && <p>关联任务 {count(activeCloud.analysis_summary.job_count)} · 事件 {count(activeCloud.analysis_summary.event_count)}</p>}<p>仅反映服务端已关联的分析，不代表全部会话均已分析。</p></div></div>
          <div className="task-result-summary"><span>新增 <strong>{count(activeCloud.message_count)}</strong> 条</span><span>已存在 <strong>{count(activeCloud.duplicate_count)}</strong> 条</span><span>会话 <strong>{count(activeCloud.conversation_count)}</strong></span><span>图片 <strong>{count(activeCloud.image_count)}</strong></span></div>
        </div>
        {linked.filter(job => !pollError && settings && cloudRetryCandidate(settings, activeCloud, userId, deviceId, job.id)).map(job => <div className="task-retry" key={job.id}><p>已匹配当前用户和本机任务归属；重试前会重新核验云端状态及原包内容。</p><button type="button" onClick={() => void prepareRetry(job, activeCloud)} disabled={retrying}>重试本机上传{job.manual_group ? ` · 第 ${(job.manual_index ?? 0) + 1} 批` : ''}</button></div>)}
        {activeCloud.status === 'failed' && !linked.some(job => !pollError && settings && cloudRetryCandidate(settings, activeCloud, userId, deviceId, job.id)) && <p className="task-meta">未能确认当前用户、本机任务归属或原包，无法安全重试。请先查看云端记录，必要时重新创建采集。</p>}
        {retryError && <p className="task-error" role="alert">{retryError}</p>}{retryDialog}
        <p className="task-meta">云端任务会自动刷新；列表变化时会自动对齐。</p>
        <section className="task-history" aria-label="已加载的云端任务"><header><h3>已加载的云端任务 <span>· 上传与解析记录</span></h3><small>{cloud.length} 条{pagination?.next ? ' · 还有更多' : ''}</small></header><div className="task-history-scroll"><table><thead><tr><th>批次</th><th>提交时间</th><th>解析结果</th><th>状态</th></tr></thead><tbody>{cloud.slice(0, 6).map(batch => <tr key={batch.id} className={batch.id === activeCloud.id ? 'selected' : ''}><td><button type="button" onClick={() => select(`cloud:${batch.id}`)}>#{batch.id}</button></td><td>{timeLabel(batch.created_at)}</td><td>新增 {count(batch.message_count)} 条</td><td><TaskStatus value={batch.status}/></td></tr>)}</tbody></table></div><footer><span>仅展示已加载记录的前 6 条</span><a href="#task-items">查看列表 →</a></footer></section>
      </div> : activeLocal ? <div className="task-detail-body">
        <header className="task-overview"><div className="task-overview-copy"><p className="task-kicker">本机队列 · 未关联云端批次<span aria-hidden="true">·</span><TaskStatus value={localGroupStatus(parts)}/></p><h2>{localTitle(activeLocal)}</h2><p className="task-meta"><time dateTime={activeLocal.created_at || activeLocal.updated_at}>{timeLabel(activeLocal.created_at || activeLocal.updated_at)}</time><span>空间 {activeLocal.space_id}</span></p><p className="task-account">微信账号 <span>{activeLocal.account_id}</span></p></div><TaskStatus value={localGroupStatus(parts)}/></header>
        <div className="task-stages"><h3>执行流程</h3><div className="task-stage"><span className="task-stage-dot" aria-hidden="true">·</span><div><strong>本机打包与上传</strong><p>这些队列项尚未保存云端批次 ID，不能据此判断云端是否存在记录或是否完成。</p></div><TaskStatus value={localGroupStatus(parts)}/></div><div className="task-stage"><span className="task-stage-dot" aria-hidden="true">·</span><div><strong>云端解析与关联分析</strong><p>无法从本机队列确认，请查看云端记录。</p></div></div></div>
        <div className="task-group-summary"><h3>本机未关联批次{activeLocal.manual_group ? ` · ${parts.length} 条` : ''}</h3>{activeLocal.manual_group && <p>同一手动任务已有的云端批次在云端列表中分别显示；此处保留所有未关联批次队列项。</p>}{parts.map(job => <div className="task-batch" key={job.id}><span className="task-batch-number">{job.manual_group ? `第 ${(job.manual_index ?? 0) + 1} / ${job.manual_total || '未知'} 批` : '本机任务'}</span><span className="task-batch-count">{localSelectionLabel(job)}</span><TaskStatus value={job.status}/>{settings && retryCandidate(settings, job.id, userId, deviceId) && <button type="button" onClick={() => void prepareRetry(job)} disabled={retrying}>重试{job.manual_group ? '该批' : '上传'}</button>}{job.status === 'failed' && job.create_started !== false && <p className="task-error">云端是否已创建批次无法确认，请先查看云端记录并重新确认，暂不提供重试。</p>}{job.message && <p className={job.status === 'failed' ? 'task-error' : 'task-meta'}>{job.message}</p>}</div>)}
          <p>重试前会再次核对任务归属、云端登录用户及目标空间。</p>
          {retryError && <p className="task-error" role="alert">{retryError}</p>}
        </div>
        {retryDialog}
      </div> : <div className="task-detail-empty"><h2>从左侧选择任务</h2><p>{error ? '无法获取云端记录，将自动重试。' : '上传后，可以在这里查看任务状态。'}</p></div>}
    </section>}
  </div>;
}
