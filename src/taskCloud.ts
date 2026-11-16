import { apiBaseUrl } from './serviceUrl.ts';

export type UploadScope = { usernames: string[]; since: number | null; until: number | null; base_url: string; message_selection?: Record<string, unknown[]> };
export type UploadOwner = { base_url: string; space_id: string; account_id: string; user_id: number; device_id: string };
export type UploadJob = { id: string; account_id: string; space_id: string; batch_id: number | null; status: string; message: string; created_at: string; updated_at: string; scope?: UploadScope | null; owner?: UploadOwner | null; create_started?: boolean | null; manual_group?: string | null; manual_index?: number; manual_total?: number };
export type TaskSettings = { active_account_id?: string; devify?: { base_url?: string; active_space_id?: string | null; device_id?: string; device_label?: string }; upload_queue?: UploadJob[] };
export type AnalysisSummary = { state?: string; job_count?: number; event_count?: number; running?: boolean; failed?: boolean; partial?: boolean; retried?: boolean; duration_seconds?: number | null };
export type CloudBatch = { id: number; uuid: string; space_id: string; uploaded_by_id: number | null; source_account_id: string; device_id: string; platform: string; trigger: string; status: string; error_message: string; message_count: number; duplicate_count: number; conversation_count: number; image_count: number; analysis_summary: AnalysisSummary | null; created_at: string; updated_at: string; archive_received_at: string | null; parse_started_at: string | null; completed_at: string | null };
export type CloudPage = { list: CloudBatch[]; pagination: { total: number; page: number; page_size: number; next: string | null; previous: string | null } };

export function settingsScope(settings: TaskSettings): string {
  return JSON.stringify([apiBaseUrl(settings.devify?.base_url ?? ''), settings.devify?.active_space_id ?? null, settings.devify?.device_id ?? '', settings.active_account_id ?? '']);
}
function normalizedBase(value: unknown): string {
  if (typeof value !== 'string' || value !== value.trim()) return '';
  return apiBaseUrl(value);
}
export function jobInScope(job: UploadJob, settings: TaskSettings): boolean {
  const base = normalizedBase(settings.devify?.base_url);
  const scope = job.scope;
  return !!base && !!settings.devify?.active_space_id && job.space_id === settings.devify.active_space_id &&
    typeof job.id === 'string' && !!job.id && typeof job.account_id === 'string' && !!job.account_id.trim() &&
    !!scope && normalizedBase(scope.base_url) === base && Array.isArray(scope.usernames) && scope.usernames.length > 0 &&
    Object.prototype.hasOwnProperty.call(scope, 'since') && Object.prototype.hasOwnProperty.call(scope, 'until') &&
    scope.usernames.every(name => typeof name === 'string' && !!name.trim()) &&
    (scope.since == null || typeof scope.since === 'number' && Number.isFinite(scope.since)) &&
    (scope.until == null || typeof scope.until === 'number' && Number.isFinite(scope.until)) &&
    (scope.since == null || scope.until == null || scope.since < scope.until);
}
export function hasCompleteOwner(job: UploadJob): boolean {
  const owner = job.owner;
  return !!owner && !!normalizedBase(owner.base_url) && typeof owner.space_id === 'string' && !!owner.space_id.trim() && owner.space_id === owner.space_id.trim() &&
    typeof owner.account_id === 'string' && !!owner.account_id && owner.account_id === owner.account_id.trim() &&
    Number.isSafeInteger(owner.user_id) && owner.user_id > 0 && typeof owner.device_id === 'string' &&
    !!owner.device_id && owner.device_id === owner.device_id.trim();
}
export function verifiedDevice(identity: unknown, userId: number | null): string | null {
  if (!identity || typeof identity !== 'object' || !Number.isSafeInteger(userId) || !userId || userId < 1) return null;
  const value = identity as { user_id?: unknown; device_id?: unknown };
  return value.user_id === userId && typeof value.device_id === 'string' && !!value.device_id &&
    value.device_id === value.device_id.trim() ? value.device_id : null;
}
// Revalidate on both sides of each preview/confirmation phase. A transient error,
// account change, or device change is never interpreted as a valid identity.
export async function retryIdentityIsCurrent(
  userId: number | null, deviceId: string | null,
  readUser: () => Promise<number | null>, readDevice: (userId: number | null) => Promise<string | null>,
): Promise<boolean> {
  if (!userId || !deviceId) return false;
  try {
    const before = await readUser();
    if (before !== userId) return false;
    const device = await readDevice(before);
    if (device !== deviceId) return false;
    return await readUser() === userId;
  } catch { return false; }
}
export function ownerMatches(job: UploadJob, settings: TaskSettings, userId: number | null, deviceId: string | null): boolean {
  const owner = job.owner;
  return hasCompleteOwner(job) && !!owner && jobInScope(job, settings) && Number.isSafeInteger(userId) &&
    owner.user_id === userId && owner.space_id === job.space_id && owner.account_id === job.account_id &&
    owner.account_id === settings.active_account_id && !!deviceId && owner.device_id === deviceId &&
    normalizedBase(owner.base_url) === normalizedBase(settings.devify?.base_url);
}
export function localUnbatched(settings: TaskSettings): UploadJob[] {
  return (settings.upload_queue ?? []).filter(job => job.batch_id == null && jobInScope(job, settings))
    .sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));
}
// Never disclose job details under a different or unverified cloud identity.
export function visibleLocalJobs(settings: TaskSettings, userId: number | null, deviceId: string | null): UploadJob[] {
  return localUnbatched(settings).filter(job => ownerMatches(job, settings, userId, deviceId));
}
export function localQueueCounts(settings: TaskSettings, userId: number | null, deviceId: string | null): { hidden: number; unknown: number } {
  const jobs = (settings.upload_queue ?? []).filter(job => job.batch_id == null);
  const visible = visibleLocalJobs(settings, userId, deviceId).length;
  const unknown = jobs.filter(job => !hasCompleteOwner(job) || !job.scope || !normalizedBase(job.scope.base_url)).length;
  return { hidden: jobs.length - visible, unknown };
}
export function linkedLocalJobs(batch: CloudBatch, settings: TaskSettings, deviceId: string | null): UploadJob[] {
  if (!deviceId || !batch.device_id || deviceId !== batch.device_id || !batch.source_account_id) return [];
  return (settings.upload_queue ?? []).filter(job => jobInScope(job, settings) && job.batch_id === batch.id &&
    job.space_id === batch.space_id && job.account_id === batch.source_account_id);
}
export function mergeCloudPages(current: CloudBatch[], incoming: CloudBatch[], spaceId: string): CloudBatch[] {
  const byId = new Map(current.filter(batch => batch.space_id === spaceId).map(batch => [batch.id, batch]));
  for (const batch of incoming) if (batch.space_id === spaceId && Number.isSafeInteger(batch.id) && batch.id > 0) byId.set(batch.id, batch);
  return [...byId.values()];
}
export function updateLoadedCloud(current: CloudBatch[], refreshed: CloudBatch[], expected: number[]): { list: CloudBatch[]; missing: number[] } {
  const latest = new Map(refreshed.map(batch => [batch.id, batch]));
  return { list: current.map(batch => latest.get(batch.id) ?? batch), missing: expected.filter(id => !latest.has(id)) };
}
export function validateCloudPage(page: CloudPage, pageNumber: number, spaceId: string): void {
  if (!Array.isArray(page?.list) || !page.pagination || page.pagination.page !== pageNumber ||
    !Number.isSafeInteger(page.pagination.total) || page.pagination.total < 0 ||
    !Number.isSafeInteger(page.pagination.page_size) || page.pagination.page_size < 1 ||
    page.list.some(batch => batch.space_id !== spaceId || !Number.isSafeInteger(batch.id) || batch.id <= 0)) {
    throw new Error('服务端分页响应无效或不属于当前空间');
  }
}
export function pollPagePlan(pageCount: number, selectedPage: number | undefined, activePages: number[], cursor: number): number[] {
  if (!Number.isSafeInteger(pageCount) || pageCount < 1) return [];
  const pages: number[] = [];
  const add = (page: number | undefined) => { if (page && page >= 1 && page <= pageCount && !pages.includes(page) && pages.length < 3) pages.push(page); };
  add(1); add(selectedPage);
  const active = [...new Set(activePages)].filter(page => page >= 1 && page <= pageCount).sort((a, b) => a - b);
  if (active.length && cursor % 2 === 0) add(active[Math.floor(cursor / 2) % active.length]);
  const rotation = active.length ? Math.floor(cursor / 2) : cursor;
  for (let n = 0; n < Math.min(pageCount, 3); n += 1) add((rotation + n) % pageCount + 1);
  return pages;
}
export async function fetchCloudPages(numbers: number[], spaceId: string, fetchPage: (page: number) => Promise<CloudPage>): Promise<CloudPage[]> {
  if (numbers.length > 3 || numbers.some(number => !Number.isSafeInteger(number) || number < 1) || new Set(numbers).size !== numbers.length) throw new Error('分页范围无效');
  // Wait for every request even when one fails; a new poll must not overlap
  // surviving requests from a failed Promise.all batch.
  const results = await Promise.allSettled(numbers.map(async number => { const page = await fetchPage(number); validateCloudPage(page, number, spaceId); return page; }));
  const failed = results.find(result => result.status === 'rejected');
  if (failed?.status === 'rejected') throw failed.reason;
  return results.map(result => (result as PromiseFulfilledResult<CloudPage>).value);
}
export function sameCloudContext(expectedScope: string, expectedUser: number | null, actualScope: string, actualUser: number | null): boolean {
  return !!expectedScope && expectedScope === actualScope && expectedUser != null && expectedUser === actualUser;
}
export function requestIsCurrent(generation: number, currentGeneration: number, revision: number, currentRevision: number, paused: boolean): boolean {
  return generation === currentGeneration && revision === currentRevision && !paused;
}
// Offset pagination has no server snapshot token. Only publish a rebuilt prefix after
// two complete, identical membership scans; keep the old snapshot on any drift.
export type CloudRebuild = { wanted: number; target: number; phase: 'collect' | 'verify'; pages: CloudPage[]; verified: CloudPage[] };
export function refreshInterval(active: boolean, rebuilding: boolean): number {
  return rebuilding ? 2000 : active ? 10000 : 30000;
}
export function sameCloudLayout(a: CloudPage, b: CloudPage): boolean {
  return a.pagination.page === b.pagination.page && a.pagination.total === b.pagination.total &&
    a.pagination.page_size === b.pagination.page_size && !!a.pagination.next === !!b.pagination.next &&
    a.list.length === b.list.length && a.list.every((item, index) => item.id === b.list[index]?.id);
}
export function validateCloudPrefixPage(page: CloudPage, number: number, spaceId: string): void {
  validateCloudPage(page, number, spaceId);
  const { total, page_size: size, next } = page.pagination;
  const last = Math.max(1, Math.ceil(total / size));
  const count = Math.max(0, Math.min(size, total - (number - 1) * size));
  if (number > last || page.list.length !== count || !!next !== (number < last) ||
      new Set(page.list.map(item => item.id)).size !== page.list.length) {
    throw new Error('服务端分页正在变化或含重复记录');
  }
}
// Fast append only when both boundaries still match the displayed prefix.
// A changed boundary, total, or any duplicate falls back to full reconstruction.
export function appendCloudPages(snapshot: CloudPage[], incoming: CloudPage[], spaceId: string): CloudPage[] | null {
  if (!snapshot.length) return null;
  const last = snapshot.length;
  const byPage = new Map(incoming.map(page => [page.pagination.page, page]));
  if (byPage.size !== incoming.length || incoming.length !== new Set([1, last, last + 1]).size) return null;
  const first = byPage.get(1); const boundary = byPage.get(last); const added = byPage.get(last + 1);
  if (!first || !boundary || !added) return null;
  try { incoming.forEach(page => validateCloudPrefixPage(page, page.pagination.page, spaceId)); }
  catch { return null; }
  if (!sameCloudLayout(snapshot[0], first) || !sameCloudLayout(snapshot[last - 1], boundary) ||
      added.pagination.total !== first.pagination.total || added.pagination.page_size !== first.pagination.page_size) return null;
  const seen = new Set(snapshot.flatMap(page => page.list.map(item => item.id)));
  if (added.list.some(item => seen.has(item.id))) return null;
  const updated = snapshot.map(page => byPage.get(page.pagination.page) ?? page);
  return [...updated, added];
}
export function startCloudRebuild(wanted: number): CloudRebuild {
  return { wanted: Math.max(1, wanted), target: 0, phase: 'collect', pages: [], verified: [] };
}
export function rebuildNextPages(state: CloudRebuild): number[] {
  const done = state.phase === 'collect' ? state.pages.length : state.verified.length;
  const target = state.target || 1; // Determine the last valid page from page one first.
  return Array.from({ length: Math.min(3, target - done) }, (_, index) => done + index + 1);
}
export function advanceCloudRebuild(state: CloudRebuild, incoming: CloudPage[], spaceId: string): CloudRebuild {
  const numbers = rebuildNextPages(state);
  if (incoming.length !== numbers.length) throw new Error('分页重建响应不完整');
  const next = { ...state, pages: [...state.pages], verified: [...state.verified] };
  for (let i = 0; i < numbers.length; i++) {
    const page = incoming[i];
    validateCloudPrefixPage(page, numbers[i], spaceId);
    if (!next.target) next.target = Math.min(next.wanted, Math.max(1, Math.ceil(page.pagination.total / page.pagination.page_size)));
    const first = next.pages[0];
    if (first && (first.pagination.total !== page.pagination.total || first.pagination.page_size !== page.pagination.page_size)) throw new Error('分页总数已变化');
    if (next.phase === 'collect') {
      if (next.pages.some(previous => previous.list.some(item => page.list.some(batch => batch.id === item.id)))) throw new Error('分页含重复记录');
      next.pages.push(page);
    } else {
      if (!sameCloudLayout(next.pages[numbers[i] - 1], page)) throw new Error('分页内容已变化');
      next.verified.push(page);
    }
  }
  if (next.phase === 'collect' && next.pages.length === next.target) next.phase = 'verify';
  return next;
}
export function rebuildComplete(state: CloudRebuild): boolean {
  return state.phase === 'verify' && state.verified.length === state.target;
}
export function cloudNeedsPolling(batches: CloudBatch[]): boolean {
  return batches.some(batch => ['created', 'queued', 'pending', 'running', 'uploading', 'submitted', 'processing'].includes(batch.status) ||
    !!batch.analysis_summary && (batch.analysis_summary.running === true || batch.analysis_summary.state === 'running'));
}
export function localGroupKey(job: UploadJob): string {
  return job.manual_group ? `group:${JSON.stringify([job.account_id, job.manual_group])}` : `job:${job.id}`;
}
export function triggerLabel(trigger: string | null | undefined): string {
  if (trigger === 'manual') return '手动';
  if (trigger === 'scheduled') return '计划';
  return trigger && trigger !== 'unknown' ? `未知类型（${trigger}）` : '未知';
}
export function analysisLabel(summary: AnalysisSummary | null | undefined): string {
  if (!summary || !(typeof summary.job_count === 'number' && summary.job_count > 0)) return '服务端未报告已关联的分析任务';
  const labels: Record<string, string> = { running: '进行中', failed: '失败', partial: '部分完成', ready: '就绪' };
  return labels[summary.state ?? ''] ?? '状态未知';
}
export function retryCandidate(settings: TaskSettings, id: string, userId: number | null, deviceId: string | null): UploadJob | undefined {
  // A missing or true flag means a create request may already have succeeded.
  // Retrying could duplicate an upload if its package or login has since changed.
  return visibleLocalJobs(settings, userId, deviceId).find(job => job.id === id && job.status === 'failed' && job.create_started === false);
}
export function cloudRetryCandidate(settings: TaskSettings, batch: CloudBatch, userId: unknown, deviceId: string | null, id: string): UploadJob | undefined {
  if (!Number.isSafeInteger(userId) || (userId as number) <= 0 || batch.uploaded_by_id !== userId || batch.status !== 'failed') return undefined;
  // Remote uploader/device establishes cloud status, not ownership of the local
  // archive. A complete matching owner is required before retry is offered;
  // the backend still verifies the original package against the server key.
  return linkedLocalJobs(batch, settings, deviceId).find(job => job.id === id && job.status === 'failed' && ownerMatches(job, settings, userId as number, deviceId));
}
export function sameRetryJob(a: UploadJob, b: UploadJob): boolean {
  return a.id === b.id && a.account_id === b.account_id && a.space_id === b.space_id && a.batch_id === b.batch_id && a.create_started === b.create_started && JSON.stringify(a.scope) === JSON.stringify(b.scope) && JSON.stringify(a.owner) === JSON.stringify(b.owner);
}
