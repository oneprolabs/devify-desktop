import assert from 'node:assert/strict';
import { mock } from 'node:test';

let next;
let calls = 0;
let options;
const installCalls = [];
let checkUploads;
let runInstall;
mock.module('@tauri-apps/api/core', { namedExports: {
  isTauri: () => true,
  invoke: async (command, args) => {
    installCalls.push([command, args]);
    if (command === 'update_upload_accounts') return checkUploads();
    if (command === 'update_begin_install' || command === 'update_cancel_install') return;
    throw Error(command);
  }
} });
mock.module('@tauri-apps/plugin-process', { namedExports: {
  relaunch: async () => { installCalls.push(['relaunch']); }
} });
mock.module('@tauri-apps/plugin-updater', { namedExports: {
  check: async (input) => { calls++; options = input; return typeof next === 'function' ? next() : next; }
} });
mock.module('react', { namedExports: { useSyncExternalStore: (_, getSnapshot) => getSnapshot() } });

const { checkForUpdates, downloadUpdate, installUpdate, useUpdater } = await import('../src/features/updates/updater.ts');
const closed = [];
const downloaded = [];
const offer = (version) => ({
  version, currentVersion: '0.1.8', body: 'Release ' + version,
  close: async () => { closed.push(version); },
  download: async () => { downloaded.push(version); },
  install: () => runInstall()
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

let resolveRefresh;
next = () => new Promise(done => { resolveRefresh = done; });
const refresh = checkForUpdates(true);
await downloadUpdate();
// Downloading uses the offer that was already checked. A second manifest request
// would make a slow or unavailable update service look like an inert button.
assert.deepEqual(downloaded, ['0.1.10']);
assert.equal(calls, 4);
resolveRefresh(offer('0.1.11'));
await refresh;
// A refresh that was already in flight must not overwrite a download in progress
// or close the object selected by that download.
assert.equal(useUpdater().status, 'downloaded');
assert.equal(useUpdater().version, '0.1.10');
let finishProbe;
checkUploads = () => new Promise(done => { finishProbe = done; });
const blockedInstall = installUpdate();
assert.equal(useUpdater().status, 'preparing');
await installUpdate();
await installUpdate(true);
await checkForUpdates();
assert.equal(calls, 4, 'manifest checks stay paused while preparing');
assert.deepEqual(installCalls, [['update_upload_accounts', undefined]], 'repeated actions cannot start a second install');
finishProbe(['test-account']);
await blockedInstall;
assert.equal(useUpdater().status, 'downloaded');
assert.deepEqual(useUpdater().blockers, ['test-account']);

runInstall = async () => { throw Error('simulated install failure'); };
await installUpdate(true);
assert.equal(useUpdater().status, 'downloaded');
assert.match(useUpdater().error, /simulated install failure/);
assert.deepEqual(installCalls.slice(1), [
  ['update_begin_install', { force: true }],
  ['update_cancel_install', undefined],
]);

let finishInstall;
runInstall = () => new Promise(done => { finishInstall = done; });
const retry = installUpdate();
assert.equal(useUpdater().status, 'preparing');
assert.equal(useUpdater().error, undefined);
finishProbe([]);
await new Promise(resolve => setImmediate(resolve));
assert.equal(useUpdater().status, 'installing');
finishInstall();
await retry;
assert.deepEqual(installCalls.slice(3), [
  ['update_upload_accounts', undefined],
  ['update_begin_install', { force: false }],
  ['relaunch'],
]);
console.log('Updater refresh, preparation, blockers, failure and retry checks passed');
