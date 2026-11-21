import { useEffect, useState } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { desktopInvoke } from '../../desktopInvoke';

export type RuntimeJob = { id: string; account_id: string; status: string; message: string; updated_at: string; batch_id?: number; kind?:'summary';title?:string;engine?:string;caller?:string;initiator_id?:number;scope?: { usernames: string[]; message_selection?: Record<string, unknown[]> } };
export type RuntimeSource = { account_id: string; platform: string; setup_complete: boolean; backup_enabled: boolean; wizard_step?: 'sessions' | 'backup' | null };
export type TaskRuntime = { context?: string | null; revision?: number; sources?: RuntimeSource[]; paused: boolean; running: boolean; pending_config: number; error?: string | null; jobs: RuntimeJob[] };
let snapshot: TaskRuntime = { paused: false, running: false, pending_config: 0, jobs: [] };
const subscribers = new Set<(value: TaskRuntime) => void>();
let started = false;
let epoch = 0;
function publish(value: TaskRuntime) { if (value.context && value.context === snapshot.context && (value.revision ?? 0) < (snapshot.revision ?? 0)) return; snapshot = value; for (const notify of subscribers) notify(value); }
export async function refreshTaskRuntime() {
  const request = epoch;
  if (isTauri()) { const value = await desktopInvoke<TaskRuntime>('task_runtime_status'); if (request === epoch) publish(value); }
}
export function resetTaskRuntime(refresh = false) {
  epoch += 1;
  publish({ context: null, paused: false, running: false, pending_config: 0, jobs: [] });
  if (refresh) void refreshTaskRuntime().catch(() => {});
}
export function useTaskRuntime() {
  const [value, setValue] = useState(snapshot);
  useEffect(() => {
    subscribers.add(setValue);
    if (!started && isTauri()) {
      started = true;
      void listen<TaskRuntime>('task-runtime', event => { if (snapshot.context && event.payload.context === snapshot.context) publish(event.payload); }).then(() => refreshTaskRuntime()).catch(() => { started = false; });
      window.addEventListener('aimychats:auth-changed', () => {
        resetTaskRuntime(true);
      });
    }
    return () => { subscribers.delete(setValue); };
  }, []);
  return value;
}
