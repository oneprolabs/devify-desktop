import { captureTask, systemEnvironment, type ConnectionStep, type SetupCapture } from './wechatSetupState.ts';

export type WeChatAction = 'connect' | 'prepare' | 'open' | 'confirm' | 'preflight' | 'capture' | 'windows' | 'import-key' | 'import-saved' | 'decrypt' | 'complete' | 'result' | 'restart';
export type StepCheck = { kind: 'ready' } | {
  kind: 'missing' | 'check-failed' | 'busy';
  message: string;
  target?: ConnectionStep | 'system';
};
export type StepReader = {
  accountId: string;
  platform: string | null;
  transactionId: string | null;
  manual: boolean;
  reuseConnection: boolean;
  captureStarted: boolean;
  capture: () => Promise<SetupCapture>;
  accountExists: () => Promise<boolean>;
  keys: () => Promise<number>;
  source: () => Promise<{ exists: boolean; secure: boolean }>;
  chats: () => Promise<void>;
};
export type StepContext = Pick<StepReader, 'platform' | 'captureStarted'>;

const missing = (message: string, target: ConnectionStep | 'system'): StepCheck => ({ kind: 'missing', message, target });
/** Native validation remains authoritative if prerequisites change after our read. */
export function capturePrerequisiteFailure(reason: unknown): StepCheck | undefined {
  if (!reason || typeof reason !== 'object' || !('code' in reason)) return;
  switch (reason.code) {
    case 'in_place_capture_not_prepared': return missing('微信准备结果已失效，请重新准备微信。', 'prepare');
    case 'capture_preflight_required':
    case 'capture_preflight_stale': return missing('授权检查结果缺失或已失效，请返回「授权连接」重新检查。', 'authorize');
    case 'debug_wechat_not_running':
    case 'capture_target_changed':
    case 'in_place_capture_target_mismatch':
    case 'in_place_debug_identity_unknown':
    case 'external_install_conflict': return missing('微信进程或安装已变化，请先恢复微信，再重新连接。', 'recover');
    case 'account_probe_invalid':
    case 'probe_database_required':
    case 'probe_database_invalid': return missing('当前账号的聊天数据库无法用于连接，请重新确认账号数据。', 'account');
    case 'lldb_runtime_unavailable': return missing('系统工具暂不可用，请返回系统检查，修复后继续。', 'system');
    case 'in_place_capture_owner_changed': return { kind: 'check-failed', message: '连接步骤已变化，请重新检查；仍不一致时，请先恢复微信。', target: 'recover' };
  }
}
const captureStep = (capture: SetupCapture): ConnectionStep => {
  const task = captureTask(capture);
  return task === 'login' ? 'login' : task === 'preflight' ? 'authorize' : task === 'logout' ? 'logout'
    : task === 'monitor' || task === 'resume' ? 'relogin' : task === 'prepare' ? 'prepare' : 'recover';
};

/** Read only the prerequisites needed by this action. A failed probe is never evidence of absence. */
export async function checkWeChatStep(action: WeChatAction, read: StepReader): Promise<StepCheck> {
  try {
    if (!read.platform) return missing('请先完成系统检查，再继续连接。', 'system');
    if (read.platform === 'macos') {
      const capture = await read.capture();
      const task = captureTask(capture);
      if (task === 'recover' || task === 'unknown') {
        // Recovery is intentionally reachable even if permissions/account data cannot be read.
        if (action === 'connect') return { kind: 'ready' };
        return missing('上次连接尚未正常结束，请先处理微信恢复。', 'recover');
      }
      if (action !== 'connect' && capture.stage === 'capturing' && !capture.resume_available) {
        return { kind: 'busy', message: '连接任务仍在执行，请等待结果；无需重新准备微信。' };
      }
      const captureActions = ['open', 'confirm', 'preflight', 'capture'];
      if (captureActions.includes(action)) {
        if (!capture.pending) return missing(
          read.captureStarted ? '本次连接记录已结束，请先检查连接结果。' : '没有找到已准备好的微信，请返回「准备微信」。',
          read.captureStarted ? 'relogin' : 'prepare',
        );
        if (read.transactionId && capture.transaction_id !== read.transactionId) {
          return missing('连接记录已变化，请先处理微信恢复，再重新连接。', 'recover');
        }
        const allowed = action === 'open' || action === 'confirm' ? ['awaiting_manual_launch', 'launched']
          : action === 'preflight' ? ['launched', 'preflight_passed'] : ['preflight_passed', ...(capture.resume_available ? ['capturing'] : [])];
        if (!allowed.includes(capture.stage)) return missing('前一步尚未完成，请返回对应步骤后继续。', captureStep(capture));
      } else if (capture.pending && action !== 'connect') {
        return missing('微信连接尚未结束，请先完成当前连接，或取消连接并恢复微信。', captureStep(capture));
      }
      if (action === 'prepare') {
        const environment = systemEnvironment(capture, read.platform, false);
        if (!environment.ready) return missing(environment.problem, 'system');
      }
      if (action === 'connect') {
        const environment = systemEnvironment(capture, read.platform, read.manual || read.reuseConnection);
        if (!environment.ready) return missing(environment.problem, 'system');
      }
    }
    if (['open', 'confirm', 'preflight'].includes(action)) return { kind: 'ready' };
    if (!read.accountId || !await read.accountExists()) return missing('未找到所选微信账号的数据，请重新确认账号；若未登录过，请先在微信中登录一次。', action === 'connect' ? 'system' : 'account');
    if (action === 'import-saved') {
      const source = await read.source();
      if (!source.exists) return missing('已保存的连接文件不存在，请重新选择文件或准备微信。', read.manual ? 'file' : 'prepare');
      if (!source.secure) return missing('连接文件的访问权限需要修复，请返回「校验连接」。', 'verify');
    }
    if (['decrypt', 'complete', 'result'].includes(action)) {
      const keys = await read.keys();
      if (!keys) {
        const source = await read.source();
        if (source.exists) return action === 'result' ? { kind: 'ready' } : missing('当前账号还没有通过校验的连接结果，请先校验连接文件。', 'verify');
        return missing('当前账号没有可用的连接结果，请重新选择文件或准备微信。', read.manual ? 'file' : 'prepare');
      }
    }
    if (action === 'complete') {
      try { await read.chats(); }
      catch { return { kind: 'check-failed', message: '未能确认聊天记录是否可读，请重新检查；仍失败时，请返回「读取聊天记录」。', target: 'read' }; }
    }
    return { kind: 'ready' };
  } catch {
    return { kind: 'check-failed', message: '未能完成前置检查，请重新检查。当前进度已保留。' };
  }
}
