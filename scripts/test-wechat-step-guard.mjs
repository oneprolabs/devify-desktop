import assert from 'node:assert/strict';
import { capturePrerequisiteFailure, checkWeChatStep } from '../src/wechatStepGuard.ts';

const idle = { pending: false, stage: 'idle', needs_cleanup: false, recovery_error_code: null, prepared_process_exited: null, resume_available: false, platform_supported: true, lldb_available: true, wechat_official: true, user_can_write: true, transaction_id: null };
let capture = idle, account = true, keys = 2, source = { exists: true, secure: true }, calls = [];
const read = {
  accountId: 'A', platform: 'macos', transactionId: 'one', manual: false, reuseConnection: false, captureStarted: false,
  capture: async () => { calls.push('capture'); return capture; },
  accountExists: async () => { calls.push('account'); return account; },
  keys: async () => { calls.push('keys'); return keys; },
  source: async () => { calls.push('source'); return source; },
  chats: async () => { calls.push('chats'); },
};
const check = async (action, changes = {}) => { calls = []; return checkWeChatStep(action, { ...read, ...changes }); };
assert.equal((await check('prepare')).kind, 'ready');
assert.deepEqual(calls, ['capture', 'account'], 'prepare checks the system result and selected account, without rechecking permissions');
for (const action of ['open', 'confirm', 'preflight', 'capture']) {
  assert.equal((await check(action)).target, 'prepare', `${action} cannot run after its transaction disappeared`);
  assert.deepEqual(calls, ['capture'], 'missing prerequisites stop later checks');
}
assert.equal((await check('capture', { captureStarted: true })).target, 'relogin', 'an ended attempt must check results before proposing a fresh capture');
capture = { ...idle, pending: true, needs_cleanup: true, stage: 'awaiting_manual_launch', transaction_id: 'one', wechat_official: false, wechat_ad_hoc: true };
assert.equal((await check('open')).kind, 'ready');
assert.deepEqual(calls, ['capture'], 'opening a prepared app does not scan data or permissions again');
assert.equal((await check('preflight')).target, 'login');
assert.equal((await check('capture')).target, 'login');
assert.equal((await check('decrypt')).target, 'login', 'saved keys cannot skip an unfinished capture');
capture = { ...capture, stage: 'launched' };
assert.equal((await check('preflight')).kind, 'ready');
assert.equal((await check('capture')).target, 'authorize');
capture = { ...capture, stage: 'preflight_passed' };
assert.equal((await check('capture')).kind, 'ready');
assert.equal((await check('capture', { transactionId: 'old' })).target, 'recover', 'a different transaction cannot reuse the UI confirmation');
capture = { ...capture, stage: 'capturing' };
assert.equal((await check('capture')).kind, 'busy', 'a running task must not be started again');
assert.equal((await check('result')).kind, 'busy', 'in-progress results must not be classified as missing');
assert.equal((await check('connect')).kind, 'ready', 'reopening the wizard must allow viewing the running task');
capture = { ...capture, resume_available: true };
assert.equal((await check('capture')).kind, 'ready', 'a native-confirmed saved result may be resumed');
capture = { ...capture, prepared_process_exited: true, stage: 'preflight_passed', resume_available: false };
assert.equal((await check('capture')).target, 'recover', 'a replaced/exited process requires recovery');
capture = { ...idle, recovery_error_code: 'external_install_conflict' };
account = false;
assert.equal((await check('complete')).target, 'recover');
assert.deepEqual(calls, ['capture'], 'recovery takes priority over cached data and does not need readable account data');
assert.equal((await check('connect')).kind, 'ready', 'recovery remains reachable without permissions or an account');
capture = idle;
assert.equal((await check('decrypt')).target, 'account');
assert.deepEqual(calls, ['capture', 'account']);
assert.equal((await check('connect')).target, 'system');
account = true; keys = 0;
assert.equal((await check('decrypt')).target, 'verify', 'an existing file must be validated before reading');
assert.equal((await check('result')).kind, 'ready', 'a result file permits validation, not completion');
assert.equal((await check('complete')).target, 'verify');
assert.ok(!calls.includes('chats'), 'completion must stop before reading when keys are absent');
source = { exists: false, secure: false };
assert.equal((await check('result')).target, 'prepare');
assert.equal((await check('import-saved', { manual: true })).target, 'file');
source = { exists: true, secure: false };
assert.equal((await check('import-saved')).target, 'verify');
keys = 2;
assert.equal((await check('complete')).kind, 'ready');
assert.deepEqual(calls, ['capture', 'account', 'keys', 'chats'], 'completion verifies current-account keys and an actual session read');
for (const probe of ['capture', 'accountExists', 'keys', 'chats']) {
  const result = await check('complete', { [probe]: async () => { throw Error('probe unavailable'); } });
  assert.equal(result.kind, 'check-failed', `${probe} errors must not roll progress back as if results were missing`);
}
assert.equal((await check('windows', { platform: 'windows' })).kind, 'ready');
assert.deepEqual(calls, ['account'], 'Windows has no macOS capture prerequisite');
assert.equal((await check('prepare', { platform: null })).target, 'system');
assert.equal(capturePrerequisiteFailure({ code: 'capture_preflight_required' }).target, 'authorize', 'a missing native preflight record must guide the user back to authorization');
assert.equal(capturePrerequisiteFailure({ code: 'capture_target_changed' }).target, 'recover', 'a process replaced after the probe must not be reused');
assert.equal(capturePrerequisiteFailure({ code: 'account_probe_invalid' }).target, 'account');
assert.equal(capturePrerequisiteFailure({ code: 'in_place_capture_not_prepared' }).target, 'prepare');
assert.equal(capturePrerequisiteFailure({ code: 'command_failed' }), undefined, 'an unknown failure cannot prove a previous result was lost');
assert.equal(capturePrerequisiteFailure('temporary failure'), undefined);
console.log('WeChat per-step prerequisite, interruption, retry and completion checks passed');
