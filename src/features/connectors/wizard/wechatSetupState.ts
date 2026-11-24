export type SetupCapture = {
  pending: boolean;
  stage: string;
  needs_cleanup: boolean;
  recovery_error_code: string | null;
  prepared_process_exited: boolean | null;
  resume_available: boolean;
  monitor_ready: boolean;
  capture_phase: string | null;
  platform_supported: boolean;
  lldb_available: boolean;
  wechat_present: boolean;
  wechat_official: boolean;
  wechat_ad_hoc?: boolean;
  wechat_version?: string | null;
  wechat_build?: string | null;
  user_can_write: boolean;
  transaction_id?: string | null;
};

export type CaptureAttempt = { accountId: string; transactionId: string | null };

export function captureAttemptFor(previous: CaptureAttempt | null, accountId: string, capture: SetupCapture | null, operation: string | null): CaptureAttempt | null {
  const changedAccount = Boolean(previous?.accountId && accountId && previous.accountId !== accountId);
  const changedTransaction = Boolean(previous?.transactionId && capture?.transaction_id && previous.transactionId !== capture.transaction_id);
  const valid = changedAccount || changedTransaction ? null : previous;
  if (operation === 'start' || capture?.stage === 'capturing') {
    return { accountId: accountId || valid?.accountId || '', transactionId: capture?.transaction_id ?? valid?.transactionId ?? null };
  }
  return valid;
}

// Recovery always wins over saved keys: a usable cache cannot prove WeChat was restored.
export function captureTask(capture: SetupCapture | null) {
  if (!capture) return 'checking';
  if ((!capture.pending && capture.needs_cleanup) || capture.recovery_error_code ||
      ['external_install_conflict', 'recovery_blocked', 'invalid'].includes(capture.stage) ||
      (capture.pending && capture.prepared_process_exited && capture.stage !== 'capturing')) return 'recover';
  if (!capture.pending) return 'prepare';
  if (['awaiting_manual_launch', 'resigned', 'debug_prepared', 'backup_verified'].includes(capture.stage)) return 'login';
  if (capture.stage === 'launched') return 'preflight';
  if (capture.stage === 'preflight_passed') return 'logout';
  if (capture.stage === 'capturing') return capture.resume_available ? 'resume' : 'monitor';
  return 'unknown';
}

export type SetupChecks = {
  permission: boolean | null;
  account: boolean;
  scanning: boolean;
  keys: number;
  keyStatusLoaded: boolean;
  savedKey: boolean;
  freshKey: boolean;
  decrypted: boolean;
  capture: SetupCapture | null;
  platform: string | null;
};

export function setupChecks(state: SetupChecks): boolean[] {
  const permission = state.permission === true;
  const account = permission && state.account && !state.scanning;
  const recovery = state.capture && (state.capture.pending || state.capture.needs_cleanup || Boolean(state.capture.recovery_error_code));
  const captureChecked = state.platform !== 'macos' || state.capture !== null;
  const key = account && state.keyStatusLoaded && captureChecked && !recovery && (state.keys > 0 || state.savedKey);
  const imported = key && state.keys > 0 && !state.freshKey;
  return [permission, account, key, imported, imported && state.decrypted];
}

export function systemEnvironment(capture: SetupCapture | null, platform: string | null, reuseConnection: boolean) {
  if (!platform) return { ready: false, problem: '' };
  if (platform === 'windows') return { ready: true, problem: '' };
  if (!capture) return { ready: false, problem: '' };
  if (reuseConnection) return { ready: true, problem: '' };
  if (!capture.platform_supported) return { ready: false, problem: '当前设备不支持自动连接，请返回接入方式，选择手动导入微信密钥。' };
  if (!capture.lldb_available) return { ready: false, problem: '缺少所需的系统工具。请安装或修复 Xcode Command Line Tools，再点「重新检查」。' };
  if (!capture.wechat_official && !(capture.pending && capture.wechat_ad_hoc && captureTask(capture) !== 'recover')) return { ready: false, problem: '请将官方微信安装到「应用程序」，再点「重新检查」。' };
  if (!capture.user_can_write) return { ready: false, problem: '无法修改微信安装目录。请确认当前用户有该目录的读写权限，再点「重新检查」。' };
  return { ready: true, problem: '' };
}

export type ConnectionStep = 'account' | 'prepare' | 'login' | 'authorize' | 'logout' | 'relogin' | 'recover' | 'file' | 'windows' | 'verify' | 'read' | 'complete';

export function connectionStep(done: boolean[], capture: SetupCapture | null, manual: boolean, platform: string | null, captureStarted = false): ConnectionStep {
  const task = captureTask(capture);
  if (task === 'recover' || task === 'unknown') return 'recover';
  // Native capture restores the app and removes its transaction before the IPC
  // result/keys reach React. Keep the user's place until completion or an
  // explicit restart, rather than interpreting that intermediate idle as step 1.
  if (captureStarted && !capture?.pending && !done[2]) return 'relogin';
  if (!done[1]) return 'account';
  if (done[4]) return 'complete';
  if (done[3]) return 'read';
  if (done[2]) return 'verify';
  if (manual && !capture?.pending) return 'file';
  if (platform === 'windows') return 'windows';
  return task === 'login' ? 'login' : task === 'preflight' ? 'authorize' : task === 'logout' ? 'logout'
    : task === 'monitor' || task === 'resume' ? 'relogin' : 'prepare';
}
