import assert from 'node:assert/strict';
import { mock } from 'node:test';

let next;
let calls = 0;
let options;
mock.module('@tauri-apps/api/core', { namedExports: { isTauri: () => true, invoke: async () => [] } });
mock.module('@tauri-apps/plugin-process', { namedExports: { relaunch: async () => {} } });
mock.module('@tauri-apps/plugin-updater', { namedExports: {
  check: async (input) => { calls++; options = input; return typeof next === 'function' ? next() : next; }
} });
mock.module('react', { namedExports: { useSyncExternalStore: (_, getSnapshot) => getSnapshot() } });

const { checkForUpdates, downloadUpdate, useUpdater } = await import('../src/updater.ts');
const closed = [];
const downloaded = [];
const offer = (version) => ({
  version, currentVersion: '0.1.8', body: 'Release ' + version,
  close: async () => { closed.push(version); },
  download: async () => { downloaded.push(version); }
});

next = offer('0.1.9');
await checkForUpdates(true);
assert.equal(useUpdater().version, '0.1.9');

next = () => { throw Error('network temporary failure'); };
await checkForUpdates(true);
assert.equal(useUpdater().version, '0.1.9');
assert.equal(useUpdater().status, 'available');

let resolve;
next = () => new Promise(done => { resolve = done; });
const first = checkForUpdates(true);
const second = checkForUpdates(true);
assert.equal(useUpdater().version, '0.1.9');
assert.equal(calls, 3);
resolve(offer('0.1.10'));
await Promise.all([first, second]);
assert.equal(useUpdater().version, '0.1.10');
assert.deepEqual(closed, ['0.1.9']);
assert.equal(new Headers(options.headers).get('cache-control'), 'no-cache');

next = offer('0.1.11');
await downloadUpdate();
assert.deepEqual(downloaded, ['0.1.11']);
assert.equal(useUpdater().status, 'downloaded');
assert.equal(useUpdater().version, '0.1.11');
console.log('Updater refresh checks passed');
