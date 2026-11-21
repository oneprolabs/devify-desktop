import { invoke } from '@tauri-apps/api/core';

type Module = 'app' | 'auth' | 'upload' | 'wechat' | 'feedback' | 'ui';
export type DiagnosticEvent = {
  module: Module;
  operation: 'start' | 'open' | 'submit' | 'success' | 'failure' | 'exception';
  code?: 'COMMAND_FAILED' | 'RENDER_ERROR' | 'UNHANDLED_REJECTION';
  duration_ms?: number;
  batch_id?: number;
};

export function recordDiagnostic(event: DiagnosticEvent) {
  // Never include command arguments, error messages, user inputs or return values.
  void invoke('diagnostic_record', { event }).catch(() => {});
}

const tracked: Record<string, Module> = {
  devify_login_password: 'auth', devify_auth_status: 'auth',
  devify_sync_now: 'upload', devify_upload_package: 'upload',
  devify_upload_retry: 'upload', wechat_sync: 'wechat', wechat_decrypt: 'wechat',
  wechat_import_keys: 'wechat', wechat_import_saved_key: 'wechat',
  wechat_capture_start: 'wechat', wechat_windows_key_scan: 'wechat',
};

export async function desktopInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const module = tracked[command];
  const start = performance.now();
  if (module) recordDiagnostic({ module, operation: 'start' });
  try {
    const result = await invoke<T>(command, args);
    if (module) recordDiagnostic({ module, operation: 'success', duration_ms: Math.round(performance.now() - start) });
    return result;
  } catch (error) {
    if (module) recordDiagnostic({ module, operation: 'failure', code: 'COMMAND_FAILED', duration_ms: Math.round(performance.now() - start) });
    throw error;
  }
}

export function requestFeedback(category = 'other', page = 'app', relatedBatchIds: number[] = []) {
  window.dispatchEvent(new CustomEvent('app:feedback', { detail: { category, page, relatedBatchIds } }));
}
