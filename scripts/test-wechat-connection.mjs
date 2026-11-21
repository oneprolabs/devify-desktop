import assert from 'node:assert/strict';
import { mock } from 'node:test';

// Run the production connection hooks, including their effects and IPC boundary.
let current;
const react = {
  useState(initial) {
    const owner = current, index = owner.index++;
    if (!(index in owner.hooks)) owner.hooks[index] = typeof initial === 'function' ? initial() : initial;
    return [owner.hooks[index], value => { owner.hooks[index] = typeof value === 'function' ? value(owner.hooks[index]) : value; }];
  },
  useRef(initial) {
    const owner = current, index = owner.index++;
    if (!(index in owner.hooks)) owner.hooks[index] = { current: initial };
    return owner.hooks[index];
  },
  useMemo(fn, deps) {
    const owner = current, index = owner.index++, previous = owner.hooks[index];
    if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) owner.hooks[index] = { deps, value: fn() };
    return owner.hooks[index].value;
  },
  useCallback(fn, deps) { return react.useMemo(() => fn, deps); },
  useEffect(fn, deps) {
    const owner = current, index = owner.index++, previous = owner.hooks[index];
    if (!previous || deps.some((value, i) => !Object.is(value, previous.deps[i]))) {
      owner.effects.push(() => { previous?.cleanup?.(); owner.hooks[index] = { deps, cleanup: fn() }; });
    }
  },
};
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => { resolve = a; reject = b; });
  return { promise, resolve, reject };
};
const flush = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
const timers = new Map(), released = [];
let timerId = 0;
globalThis.window = {
  setInterval(fn, interval) { timers.set(++timerId, { fn, interval }); return timerId; },
  clearInterval(id) { timers.delete(id); },
  setTimeout(fn) { released.push(fn); return released.length; },
};
let invoke;
mock.module('react', { exports: { ...react, useSyncExternalStore: (_, get) => get() } });
mock.module(new URL('../src/shared/api/desktopInvoke.ts', import.meta.url).href, { exports: { desktopInvoke: (...args) => invoke(...args) } });
const { useWeChatConnection } = await import('../src/features/connectors/wechat/useWeChatConnection.ts');
const A = { id: 'A', dbs: [{ rel: 'session/session.db', encrypted: true }, { rel: 'message/message_0.db', encrypted: true }] };
const idle = { pending: false, stage: 'idle', platform_supported: true, lldb_available: true, wechat_official: true, user_can_write: true };
const state = { notice: '', error: '', busy: null, resets: 0, cacheClears: 0, loaded: [] };
const options = {
  account: A, accountId: 'A', accountSwitching: false, mode: 'settings', wizard: true,
  wizardStep: 'wizard-permission', wizardManual: false,
  setNotice: value => { state.notice = value; }, setError: value => { state.error = value; }, setBusy: value => { state.busy = value; },
  loadSessions: async id => { state.loaded.push(id); }, resetReader: () => { state.resets++; },
  clearMessageCache: () => { state.cacheClears++; }, clearing: { current: false },
};
let calls = [], keyRows = [], capture = idle;
let handlers = {};
invoke = async (command, args) => {
  calls.push({ command, args });
  if (handlers[command]) return handlers[command](args);
  if (command === 'wechat_capture_status') return capture;
  if (command === 'wechat_key_status') return { keys: keyRows };
  if (command === 'wechat_key_source_status') return { exists: false, secure: true };
  if (command === 'wechat_env') return { keys_path: '/synthetic/keys', cache_root: '/synthetic/cache' };
  throw Error(`unexpected IPC: ${command}`);
};
const owner = { hooks: [] };
function render() {
  current = owner; owner.index = 0; owner.effects = [];
  const value = useWeChatConnection(options);
  current = undefined; for (const effect of owner.effects) effect();
  return value;
}
let connection = render(); await flush(); connection = render();
assert.equal(calls.filter(x => x.command === 'wechat_capture_status').length, 1, 'system check probes once on entry');
assert.equal(timers.size, 0, 'idle wizard never polls permissions');
render(); await flush();
assert.equal(calls.filter(x => x.command === 'wechat_capture_status').length, 1, 'unrelated rerenders do not probe again');
await connection.initialize(); connection = render();
assert.equal(connection.env.keys_path, '/synthetic/keys');

// A delayed older probe must never overwrite a newer result or clear its loading indicator.
const old = deferred(), recent = deferred(); let probe = 0;
handlers.wechat_capture_status = () => (++probe === 1 ? old : recent).promise;
const first = connection.capture.refreshCaptureStatus();
const second = connection.capture.refreshCaptureStatus();
old.resolve({ ...idle, stage: 'stale' }); await first; connection = render();
assert.equal(connection.capture.captureStatus.stage, 'idle');
assert.equal(connection.capture.captureStatusChecking, true);
recent.resolve({ ...idle, stage: 'current' }); await second; connection = render();
assert.equal(connection.capture.captureStatus.stage, 'current');
assert.equal(connection.capture.captureStatusChecking, false);
delete handlers.wechat_capture_status;

// Rejected input stays editable; accepted input clears only after native validation.
connection.keys.setKeyText('synthetic-invalid-key'); connection = render();
handlers.wechat_import_keys = async args => { assert.equal(args.accountId, 'A'); return { matched: 0, unmatched: ['message/message_0.db'], accounts: [], message: 'rejected' }; };
await connection.keys.importKeys(); connection = render();
assert.equal(connection.keys.keyText, 'synthetic-invalid-key');
assert.match(state.error, /没有匹配/); assert.equal(state.busy, null);
handlers.wechat_import_keys = async () => ({ matched: 2, unmatched: [], accounts: [], message: 'accepted' });
keyRows = A.dbs.map(db => ({ account: 'A', rel: db.rel }));
await connection.keys.importKeys(); connection = render();
assert.equal(connection.keys.keyText, ''); assert.equal(connection.keys.keyedCount, 2);
const beforeOversize = calls.length;
await connection.keys.importKeys({ size: 2_000_001, text: () => { throw Error('must not read'); } });
assert.equal(calls.length, beforeOversize); assert.match(state.error, /过大/);

// The supported V7 Windows path still uses native memory scanning, independently of
// the removed standalone branch whose platform state was permanently null.
handlers.wechat_windows_key_scan = async args => {
  assert.deepEqual(args, { accountId: 'A' });
  return { matched: 2, already: 0, unmatched: [], accounts: [], message: 'Windows keys verified' };
};
await connection.keys.scanWindowsKeys(); connection = render();
assert.equal(connection.keys.windowsScanBusy, false);
assert.equal(state.notice, 'Windows keys verified');

// Capturing is the only wizard phase with a repeating status read, cleaned up on completion.
const captured = deferred();
handlers.wechat_capture_start = args => { assert.deepEqual(args, { accountId: 'A', timeoutSecs: 300 }); return captured.promise; };
options.wizardStep = 'wizard-connect'; connection = render();
const capturing = connection.capture.startCapture(); connection = render();
assert.deepEqual([...timers.values()].map(x => x.interval), [1500]);
captured.resolve({ matched: 2, already: 0, accounts: [], message: 'captured' }); await capturing; connection = render();
assert.equal(connection.keys.keyImport.matched, 2); assert.equal(timers.size, 0);

// Structured failures preserve prerequisite guidance and a usable retry state.
handlers.wechat_capture_prepare = async () => { throw { code: 'in_place_capture_not_prepared', message: 'prepare again' }; };
assert.equal((await connection.capture.prepareCapture()).target, 'prepare'); connection = render();
assert.equal(state.error, 'prepare again'); assert.equal(connection.capture.captureBusy, null);
handlers.wechat_capture_restore = async () => { throw { message: 'restore blocked' }; };
await connection.capture.restoreWeChat(); connection = render();
assert.equal(connection.capture.recoveryError, 'restore blocked'); assert.equal(connection.capture.captureBusy, null);

// Native keys from another account never satisfy the selected account's prerequisites.
keyRows = [{ account: 'B', rel: 'session/session.db' }];
calls = [];
assert.equal((await connection.checkStep('decrypt', { platform: 'macos', captureStarted: false }, async () => true)).target, 'prepare');
assert.ok(!calls.some(x => x.command === 'chat_sessions'));

// Decryption still forwards both options, refreshes sessions only on usable output,
// and requires both session and message databases before reporting completion.
const db = (rel, status) => ({ rel, status, account: 'A' });
handlers.wechat_decrypt = async args => { assert.deepEqual(args, { accountId: 'A', applyWal: true, onlyChat: true }); return [db('session/session.db', 'ok')]; };
await connection.decryption.decrypt(); connection = render();
assert.deepEqual(state.loaded, ['A']); assert.equal(connection.decryption.decrypted, false);
handlers.wechat_decrypt = async () => [db('session/session.db', 'cached'), db('message/message_0.db', 'ok')];
await connection.decryption.decrypt(); connection = render();
assert.equal(connection.decryption.decrypted, true);
options.accountSwitching = true; connection = render(); const noDecrypt = calls.length;
await connection.decryption.decrypt(); assert.equal(calls.length, noDecrypt);
options.accountSwitching = false; connection = render();

// Clearing holds the shared live-sync guard until after reader invalidation commits.
const clearing = deferred(); handlers.wechat_clear_cache = () => clearing.promise;
const clear = connection.decryption.clearCache();
assert.equal(options.clearing.current, true); assert.equal(state.resets, 0);
clearing.resolve('cleared'); await clear; connection = render();
assert.equal(state.resets, 1); assert.equal(state.cacheClears, 1);
assert.equal(connection.decryption.outcomes.length, 0); assert.equal(options.clearing.current, true);
released.shift()(); assert.equal(options.clearing.current, false);
handlers.wechat_clear_keys = async () => 'keys cleared';
await connection.decryption.clearKeys(); connection = render();
assert.equal(connection.keys.keyImport, null); assert.equal(state.busy, null); released.shift()();

// Account switching discards the previous result without discarding the typed input.
connection.keys.setKeyText('keep-editable-input'); connection = render();
await connection.decryption.decrypt(); connection = render();
connection.resetAccountResult(); options.account = { ...A, id: 'B' }; options.accountId = 'B'; connection = render();
assert.equal(connection.keys.keyImport, null); assert.equal(connection.decryption.outcomes.length, 0);
assert.equal(connection.keys.keyText, 'keep-editable-input');
for (const value of owner.hooks) value?.cleanup?.();
assert.equal(timers.size, 0);
console.log('WeChat connection ownership, probe ordering, polling, import, recovery, account and snapshot boundaries passed');
