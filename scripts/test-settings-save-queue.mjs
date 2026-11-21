import assert from 'node:assert/strict';
import { createSettingsSaveQueue, settingsUserRevision } from '../src/shared/state/settingsSaveQueue.ts';

const initial = { user_settings_revision: 3, language: '中文', time: '08:00', store: 'old' };
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
};
const tick = () => new Promise(resolve => setImmediate(resolve));
const patch = (key, value) => current => ({ ...current, [key]: value });

assert.throws(() => settingsUserRevision({}), /缺少有效版本/);
for (const value of [-1, 1.5, Number.MAX_SAFE_INTEGER + 1, '3', null]) {
  assert.throws(() => settingsUserRevision({ user_settings_revision: value }), /缺少有效版本/);
}
assert.throws(() => createSettingsSaveQueue({ initial: {}, save: async value => value }), /缺少有效版本/);

const calls = [];
const syncs = [];
const errors = [];
const queue = createSettingsSaveQueue({
  initial,
  save(value) { const result = deferred(); calls.push({ value, result }); return result.promise; },
  async afterSave(value) { syncs.push(value.user_settings_revision); if (syncs.length === 1) throw Error('schedule offline'); },
  onAfterSaveError(error) { errors.push(error.message); },
});
const observed = [];
const unsubscribe = queue.subscribe(state => observed.push(state));
queue.enqueue(patch('language', 'English'));
queue.enqueue(patch('time', '09:30'));
assert.equal(calls.length, 1, 'edits must not save concurrently');
assert.equal(calls[0].value.user_settings_revision, 3);
assert.equal(queue.snapshot().draft.time, '09:30', 'later draft remains visible');
calls[0].result.resolve({ ...calls[0].value, user_settings_revision: 4, store: 'background changed' });
await tick();
assert.deepEqual(errors, ['schedule offline']);
assert.equal(calls.length, 2);
assert.equal(calls[1].value.user_settings_revision, 4, 'later save must use confirmed revision');
assert.equal(calls[1].value.language, 'English');
assert.equal(calls[1].value.time, '09:30');
assert.equal(calls[1].value.store, 'background changed', 'use full confirmed response as new base');
calls[1].result.resolve({ ...calls[1].value, user_settings_revision: 5 });
await tick();
assert.deepEqual(syncs, [4, 5]);
assert.deepEqual(errors, ['schedule offline'], 'later success must not silently clear schedule error');
assert.equal(queue.snapshot().pending, 0);
assert.equal(queue.snapshot().draft.time, '09:30');
assert.equal(queue.snapshot().saving, false);
assert.ok(observed.some(state => state.pending === 2));
unsubscribe();

const failed = deferred();
let failureCalls = 0;
let failureSyncs = 0;
const conflict = createSettingsSaveQueue({
  initial,
  save: () => { failureCalls++; return failed.promise; },
  afterSave: async () => { failureSyncs++; },
});
conflict.enqueue(patch('language', 'English'));
conflict.enqueue(patch('time', '09:30'));
failed.reject('SETTINGS_CONFLICT: 账户已改变');
await tick();
assert.equal(conflict.snapshot().blocked, true);
assert.equal(conflict.snapshot().draft.language, 'English');
assert.equal(conflict.snapshot().draft.time, '09:30');
assert.equal(conflict.snapshot().pending, 2);
assert.equal(conflict.enqueue(patch('time', '10:00')), false);
assert.equal(failureCalls, 1, 'conflict must not blindly replay queued edits');
assert.equal(failureSyncs, 0, 'failed save must not sync a daily schedule');

let postSave = 0;
const invalidResponse = createSettingsSaveQueue({
  initial,
  save: async () => ({ language: 'English' }),
  afterSave: async () => { postSave++; },
});
invalidResponse.enqueue(patch('language', 'English'));
await tick();
assert.equal(invalidResponse.snapshot().blocked, true);
assert.equal(invalidResponse.snapshot().draft.language, 'English');
assert.equal(postSave, 0, 'a malformed response is not a confirmed save');

// Leaving the page detaches rendering, not already submitted saves or follow-up work.
const pending = deferred();
let followup = 0;
let renderCalls = 0;
const unmounted = createSettingsSaveQueue({ initial, save: () => pending.promise, afterSave: async () => { followup++; } });
const detach = unmounted.subscribe(() => { renderCalls++; });
unmounted.enqueue(patch('time', '11:00'));
detach();
const before = renderCalls;
pending.resolve({ ...initial, time: '11:00', user_settings_revision: 4 });
await tick();
assert.equal(followup, 1);
assert.equal(renderCalls, before);
console.log('settings save queue checks passed');
