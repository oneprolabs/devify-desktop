type Job = { status: string; manual_group?: string | null; manual_total?: number };

export function localTaskStatus(job: Job, parts: Job[]): string {
  if (!job.manual_group) return job.status;
  if (parts.some(part => part.status === 'failed')) return 'failed';
  if (parts.some(part => part.status === 'cancelled')) return 'cancelled';
  return parts.length > 0 && parts.length === (job.manual_total || parts.length) && parts.every(part => part.status === 'completed') ? 'completed' : 'processing';
}
