import type { CloudSummary, SummaryPage } from './summaryInput';

// The durable intent and the server result share this key before they share an id.
export function summaryRecordKey(row: CloudSummary): string {
  return JSON.stringify([row.engine ?? 'cloud', row.request_key || `id:${row.id}`]);
}

export function mergeSummaryRecords(...groups: CloudSummary[][]): CloudSummary[] {
  const merged = new Map<string, CloudSummary>();
  for (const row of groups.flat()) {
    const key = summaryRecordKey(row), previous = merged.get(key);
    // An enqueue response can arrive after the server list. Keep the server's
    // canonical id and status while carrying the durable local task handle.
    const keepServer = previous && (row.engine ?? 'cloud') === 'cloud'
      && typeof previous.id === 'number' && typeof row.id === 'string';
    merged.set(key, previous ? {
      ...(keepServer ? row : previous), ...(keepServer ? previous : row),
      task_id: row.task_id ?? previous.task_id,
    } : row);
  }
  return [...merged.values()].sort((a,b)=>b.created_at.localeCompare(a.created_at));
}

export function reconcileSummaryPage(result: SummaryPage) {
  const local = mergeSummaryRecords(result.local ?? []);
  const server = (result.list ?? []).map(row => {
    const task = local.find(item => (item.engine ?? 'cloud') === 'cloud'
      && (summaryRecordKey(item) === summaryRecordKey(row) || item.id === row.id));
    // A list snapshot may predate the coordinator's completed result.
    const completed = task?.status === 'ready' && (row.status === 'queued' || row.status === 'running');
    return { ...task, ...row, ...(completed ? task : {}), task_id: task?.task_id, engine: 'cloud' as const };
  });
  const localOnly = local.filter(item => !server.some(row => summaryRecordKey(row) === summaryRecordKey(item))
    && (typeof item.id === 'string' || item.engine === 'local' || result.offline
      || item.status === 'queued' || item.status === 'running'));
  return {
    rows: mergeSummaryRecords(localOnly, server),
    total: result.pagination.total + localOnly.filter(item => result.offline
      || item.engine === 'local' || typeof item.id === 'string').length,
  };
}
