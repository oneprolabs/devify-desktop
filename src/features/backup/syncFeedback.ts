export type SyncBatchStatus = { status: string; error_message?: string };

export function syncFeedback(batches: SyncBatchStatus[]): 'processing' | 'completed' | 'failed' | 'unknown' {
  if (!batches.length) return 'unknown';
  if (batches.some(batch => ['failed', 'cancelled'].includes(batch.status))) return 'failed';
  if (batches.every(batch => batch.status === 'completed')) return 'completed';
  if (batches.every(batch => ['created', 'uploading', 'processing', 'completed'].includes(batch.status))) return 'processing';
  return 'unknown';
}
