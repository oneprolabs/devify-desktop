import assert from 'node:assert/strict';
import { advanceCloudRebuild, appendCloudPages, rebuildComplete, rebuildNextPages, refreshInterval, requestIsCurrent, sameCloudContext, sameCloudLayout, startCloudRebuild, validateCloudPrefixPage, analysisLabel, cloudNeedsPolling, cloudRetryCandidate, fetchCloudPages, hasCompleteOwner, jobInScope, linkedLocalJobs, localGroupKey, localQueueCounts, localUnbatched, visibleLocalJobs, ownerMatches, verifiedDevice, mergeCloudPages, pollPagePlan, retryCandidate, retryIdentityIsCurrent, sameRetryJob, settingsScope, triggerLabel, updateLoadedCloud, validateCloudPage } from '../src/taskCloud.ts';

const scope = { base_url: 'https://example.test/', usernames: ['alice'], since: null, until: null };
const owner = { base_url: 'https://example.test/', space_id: 2, account_id: 'wx-a', user_id: 42, device_id: 'device-a' };
const job = { id: 'local', account_id: 'wx-a', space_id: 2, batch_id: null, status: 'failed', message: 'offline', created_at: '', updated_at: '', scope, owner, create_started: false };
const settings = { active_account_id: 'wx-a', devify: { base_url: 'https://example.test', active_space_id: 2, device_id: 'device-a' }, upload_queue: [job] };
const batch = (id, changes = {}) => ({ id, space_id: 2, source_account_id: 'wx-a', device_id: 'device-a', status: 'completed', ...changes });
const page = (list, number = 1, total = 2, next = null) => ({ list, pagination: { total, page: number, page_size: 20, next, previous: null } });

assert.equal(jobInScope(job, settings), true);
assert.equal(jobInScope({ ...job, scope: null }, settings), false);
assert.equal(jobInScope({ ...job, scope: { ...scope, base_url: '' } }, settings), false);
assert.equal(jobInScope({ ...job, scope: { ...scope, base_url: 'https://other.test' } }, settings), false);
assert.equal(jobInScope({ ...job, space_id: 3 }, settings), false);
assert.equal(jobInScope({ ...job, account_id: '' }, settings), false);
assert.equal(jobInScope({ ...job, scope: { base_url: scope.base_url, usernames: ['alice'] } }, settings), false);
assert.equal(jobInScope({ ...job, scope: { ...scope, usernames: [] } }, settings), false);
assert.equal(jobInScope({ ...job, scope: { ...scope, since: 4, until: 4 } }, settings), false);
assert.equal(jobInScope(job, { ...settings, devify: { ...settings.devify, active_space_id: null } }), false);
assert.notEqual(settingsScope(settings), settingsScope({ ...settings, devify: { ...settings.devify, active_space_id: 3 } }));
assert.notEqual(settingsScope(settings), settingsScope({ ...settings, devify: { ...settings.devify, base_url: 'https://other.test' } }));

const grouped1 = { ...job, id: 'g1', manual_group: 'g', manual_index: 0, manual_total: 3 };
const grouped2 = { ...job, id: 'g2', manual_group: 'g', manual_index: 1, manual_total: 3, status: 'queued' };
const grouped3 = { ...job, id: 'g3', manual_group: 'g', manual_index: 2, manual_total: 3, batch_id: 5 };
const mixed = { ...settings, upload_queue: [job, grouped1, grouped2, grouped3, { ...job, id: 'legacy', scope: null }, { ...job, id: 'other-space', space_id: 3 }] };
assert.deepEqual(localUnbatched(mixed).map(item => item.id).sort(), ['g1', 'g2', 'local']);
assert.equal(localGroupKey(grouped1), localGroupKey(grouped2));
assert.notEqual(localGroupKey(grouped1), localGroupKey({ ...grouped2, account_id: 'wx-b' }));
assert.equal(retryCandidate(mixed, 'g1', 42, 'device-a')?.id, 'g1');
assert.equal(retryCandidate(mixed, 'g2', 42, 'device-a'), undefined);
assert.equal(retryCandidate(mixed, 'g3', 42, 'device-a'), undefined);
assert.equal(retryCandidate(mixed, 'legacy', 42, 'device-a'), undefined);
assert.equal(sameRetryJob(job, { ...job }), true);
assert.equal(sameRetryJob(job, { ...job, scope: { ...scope, usernames: ['bob'] } }), false);
assert.equal(sameRetryJob(job, { ...job, batch_id: 5 }), false);
assert.equal(sameRetryJob(job, { ...job, owner: { ...owner, user_id: 43 } }), false);
assert.equal(sameRetryJob(job, { ...job, create_started: true }), false);
// Unknown create state is not evidence that the server has no batch.
for (const createStarted of [true, null, undefined, 'false', 0]) {
  const candidate = { ...job, create_started: createStarted };
  assert.equal(retryCandidate({ ...settings, upload_queue: [candidate] }, job.id, 42, 'device-a'), undefined);
}
assert.equal(retryCandidate(settings, job.id, 42, 'device-a')?.id, 'local');

// Local retries fail closed for legacy or mismatched identities. Unknown and
// other-owner jobs are retained in the queue but never exposed as current-user details.
assert.equal(hasCompleteOwner(job), true);
assert.equal(hasCompleteOwner({ ...job, owner: null }), false);
assert.equal(hasCompleteOwner({ ...job, owner: { ...owner, device_id: '' } }), false);
assert.equal(hasCompleteOwner({ ...job, owner: { ...owner, user_id: 0 } }), false);
assert.equal(ownerMatches(job, settings, 42, 'device-a'), true);
assert.equal(ownerMatches(job, settings, null, 'device-a'), false);
assert.equal(ownerMatches(job, settings, 43, 'device-a'), false);
assert.equal(ownerMatches(job, settings, 42, null), false);
assert.equal(ownerMatches(job, settings, 42, 'device-b'), false);
for (const changedOwner of [
  null,
  { ...owner, user_id: 43 },
  { ...owner, device_id: 'device-b' },
  { ...owner, account_id: 'wx-b' },
  { ...owner, space_id: 3 },
  { ...owner, base_url: 'https://other.test' },
  { ...owner, device_id: '' },
]) {
  const candidate = { ...job, owner: changedOwner };
  assert.equal(ownerMatches(candidate, settings, 42, 'device-a'), false);
  assert.equal(retryCandidate({ ...settings, upload_queue: [candidate] }, job.id, 42, 'device-a'), undefined);
}
for (const changedSettings of [
  { ...settings, active_account_id: 'wx-b' },
  { ...settings, active_account_id: '' },
  { ...settings, devify: { ...settings.devify, active_space_id: 3 } },
  { ...settings, devify: { ...settings.devify, base_url: 'https://other.test' } },
]) assert.equal(retryCandidate(changedSettings, job.id, 42, 'device-a'), undefined);
// The backend has authenticated the JWT claim; an empty configured device is not
// filled in or mutated by the frontend.
const emptyDeviceSettings = { ...settings, devify: { ...settings.devify, device_id: '' } };
const jwtIdentity = { user_id: 42, device_id: 'device-a' };
assert.equal(verifiedDevice(jwtIdentity, 42), 'device-a');
assert.equal(retryCandidate(emptyDeviceSettings, job.id, 42, verifiedDevice(jwtIdentity, 42))?.id, 'local');
assert.equal(emptyDeviceSettings.devify.device_id, '');
assert.equal(retryCandidate(emptyDeviceSettings, job.id, 42, null), undefined);
assert.equal(retryCandidate(emptyDeviceSettings, job.id, 42, verifiedDevice({ user_id: 43, device_id: 'device-a' }, 42)), undefined);
assert.equal(retryCandidate(emptyDeviceSettings, job.id, 42, verifiedDevice({ user_id: 42, device_id: 'device-b' }, 42)), undefined);
for (const badIdentity of [null, {}, { user_id: 42, device_id: '' }, { user_id: 42, device_id: ' device-a ' }, { user_id: '42', device_id: 'device-a' }]) {
  assert.equal(verifiedDevice(badIdentity, 42), null);
}
assert.equal(verifiedDevice(jwtIdentity, null), null);

// Mock a preview followed by confirmation. Both phases re-read the authenticated
// user and device; a switch or a transient identity failure blocks the retry.
let mockUser = 42;
let mockIdentity = jwtIdentity;
const readUser = async () => mockUser;
const readDevice = async userId => verifiedDevice(mockIdentity, userId);
assert.equal(await retryIdentityIsCurrent(42, 'device-a', readUser, readDevice), true); // preview
mockIdentity = { user_id: 42, device_id: 'device-b' };
assert.equal(await retryIdentityIsCurrent(42, 'device-a', readUser, readDevice), false); // confirm
mockIdentity = null;
assert.equal(await retryIdentityIsCurrent(42, 'device-a', readUser, readDevice), false);
mockIdentity = jwtIdentity; mockUser = 43;
assert.equal(await retryIdentityIsCurrent(42, 'device-a', readUser, readDevice), false);
mockUser = 42;
assert.equal(await retryIdentityIsCurrent(42, 'device-a', readUser, async () => { throw new Error('offline'); }), false);
let userReads = 0;
assert.equal(await retryIdentityIsCurrent(42, 'device-a', async () => ++userReads === 1 ? 42 : 43, readDevice), false);
assert.equal(await retryIdentityIsCurrent(null, 'device-a', readUser, readDevice), false);
assert.equal(await retryIdentityIsCurrent(42, null, readUser, readDevice), false);
const legacyNoOwner = { ...job, id: 'old-owner', owner: null, message: 'private-message' };
const legacyNoScope = { ...job, id: 'old-scope', scope: null, message: 'private-message' };
const otherUser = { ...job, id: 'other-user', owner: { ...owner, user_id: 99 }, message: 'private-message' };
const privacySettings = { ...settings, upload_queue: [job, legacyNoOwner, legacyNoScope, otherUser] };
assert.deepEqual(visibleLocalJobs(privacySettings, 42, 'device-a').map(item => item.id), ['local']);
assert.deepEqual(localQueueCounts(privacySettings, 42, 'device-a'), { hidden: 3, unknown: 2 });
assert.deepEqual(localQueueCounts(privacySettings, null, null), { hidden: 4, unknown: 2 });
assert.equal(retryCandidate(privacySettings, 'old-owner', 42, 'device-a'), undefined);
assert.equal(retryCandidate(privacySettings, 'old-scope', 42, 'device-a'), undefined);
assert.equal(retryCandidate(privacySettings, 'other-user', 42, 'device-a'), undefined);
// A cloud batch can still be displayed, but its uploader/device does not prove
// ownership of an old local archive. Never offer a retry without matching owner.
const legacyLinked = { ...job, id: 'linked-old', owner: null, batch_id: 5 };
const legacyLinkedSettings = { ...settings, upload_queue: [legacyLinked] };
const legacyBatch = batch(5, { status: 'failed', uploaded_by_id: 42 });
assert.equal(linkedLocalJobs(legacyBatch, legacyLinkedSettings, 'device-a').length, 1);
assert.equal(cloudRetryCandidate(legacyLinkedSettings, legacyBatch, 42, 'device-a', 'linked-old'), undefined);
for (const badOwner of [
  undefined, { ...owner, user_id: 99 }, { ...owner, device_id: 'device-b' },
  { ...owner, account_id: 'wx-b' }, { ...owner, space_id: 3 },
  { ...owner, base_url: 'https://other.test' }, { ...owner, device_id: '' },
]) {
  const oldSettings = { ...settings, upload_queue: [{ ...legacyLinked, owner: badOwner }] };
  assert.equal(cloudRetryCandidate(oldSettings, legacyBatch, 42, 'device-a', 'linked-old'), undefined);
}
assert.equal(cloudRetryCandidate({ ...settings, upload_queue: [{ ...legacyLinked, owner }] }, legacyBatch, 42, 'device-a', 'linked-old')?.id, 'linked-old');

const linked = { ...settings, upload_queue: [grouped3] };
assert.equal(linkedLocalJobs(batch(5), linked, 'device-a').length, 1);
assert.equal(linkedLocalJobs(batch(5, { source_account_id: 'wx-b' }), linked, 'device-a').length, 0);
assert.equal(linkedLocalJobs(batch(5, { space_id: 3 }), linked, 'device-a').length, 0);
assert.equal(linkedLocalJobs(batch(5, { device_id: 'device-b' }), linked, 'device-a').length, 0);
assert.equal(linkedLocalJobs(batch(6), linked, 'device-a').length, 0);
assert.equal(linkedLocalJobs(batch(5), linked, null).length, 0);
assert.equal(linkedLocalJobs(batch(5), { ...linked, devify: { ...linked.devify, device_id: '' } }, 'device-a').length, 1);
assert.equal(linkedLocalJobs(batch(5), { ...linked, upload_queue: [{ ...grouped3, scope: null }] }, 'device-a').length, 0);
assert.equal(linkedLocalJobs(batch(5), { ...linked, devify: { ...linked.devify, base_url: 'https://other.test' } }, 'device-a').length, 0);
const failedBatch = batch(5, { status: 'failed', uploaded_by_id: 42 });
assert.equal(cloudRetryCandidate(linked, failedBatch, 42, 'device-a', 'g3')?.id, 'g3');
assert.equal(cloudRetryCandidate(linked, failedBatch, 42, null, 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, failedBatch, 42, 'device-b', 'g3'), undefined);
assert.equal(cloudRetryCandidate({ ...linked, active_account_id: '' }, failedBatch, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate({ ...linked, active_account_id: 'wx-b' }, failedBatch, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate({ ...linked, active_account_id: undefined }, failedBatch, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, failedBatch, 43, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, failedBatch, null, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, { ...failedBatch, uploaded_by_id: null }, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, { ...failedBatch, status: 'completed' }, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate(linked, { ...failedBatch, device_id: 'device-b' }, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate({ ...linked, upload_queue: [{ ...grouped3, status: 'processing' }] }, failedBatch, 42, 'device-a', 'g3'), undefined);
assert.equal(cloudRetryCandidate({ ...linked, upload_queue: [{ ...grouped3, scope: null }] }, failedBatch, 42, 'device-a', 'g3'), undefined);

const first = page([batch(5), batch(4)], 1, 3, '?page=2');
validateCloudPage(first, 1, 2);
assert.throws(() => validateCloudPage(first, 2, 2));
assert.throws(() => validateCloudPage(page([batch(9, { space_id: 3 })]), 1, 2));
assert.throws(() => validateCloudPage(page([batch(0)]), 1, 2));
assert.throws(() => validateCloudPage({ list: [], pagination: { total: -1, page: 1, page_size: 20 } }, 1, 2));
const merged = mergeCloudPages(first.list, [batch(4, { status: 'failed' }), batch(3, { device_id: 'device-b' }), batch(10, { space_id: 3 })], 2);
assert.deepEqual(merged.map(item => item.id), [5, 4, 3]);
assert.equal(merged[1].status, 'failed');
assert.deepEqual(merged.map(item => item.device_id), ['device-a', 'device-a', 'device-b']);
assert.equal(triggerLabel('manual'), '手动');
assert.equal(triggerLabel('scheduled'), '计划');
assert.equal(triggerLabel('unknown'), '未知');
assert.equal(analysisLabel(null), '服务端未报告已关联的分析任务');
assert.equal(analysisLabel({ state: 'ready', job_count: 0 }), '服务端未报告已关联的分析任务');
assert.equal(analysisLabel({ state: 'ready' }), '服务端未报告已关联的分析任务');
assert.equal(analysisLabel({ state: 'ready', job_count: 2 }), '就绪');
assert.equal(analysisLabel({ state: 'partial', job_count: 2, partial: true }), '部分完成');
assert.equal(analysisLabel({ state: 'running', job_count: 2, running: true, failed: false }), '进行中');
for (const status of ['created', 'uploading', 'processing', 'queued', 'pending']) assert.equal(cloudNeedsPolling([batch(1, { status })]), true);
assert.equal(cloudNeedsPolling([batch(1, { status: 'completed' })]), false);
assert.equal(cloudNeedsPolling([batch(1, { status: 'completed', analysis_summary: { state: 'running', job_count: 1, running: true } })]), true);
const covered = new Set();
for (let cursor = 0; cursor < 56; cursor++) {
  const plan = pollPagePlan(26, 23, [7, 15, 24], cursor);
  assert.ok(plan.length <= 3);
  assert.ok(plan.includes(23));
  plan.forEach(number => covered.add(number));
}
assert.equal(covered.size, 26);
assert.ok(pollPagePlan(26, 23, [7, 15, 24], 0).includes(7));
assert.ok(pollPagePlan(26, 23, [7, 15, 24], 2).includes(15));
let calls = [];
const refreshed = await fetchCloudPages([1, 2], 2, async number => {
  calls.push(number);
  return number === 1 ? page([batch(5, { status: 'completed' }), batch(4)], 1, 3, '?page=2') : page([batch(4), batch(3)], 2, 3, null);
});
assert.deepEqual(calls, [1, 2]);
const updates = updateLoadedCloud([batch(5, { status: 'processing' }), batch(4), batch(99)], refreshed.flatMap(item => item.list), [5, 4]);
assert.deepEqual(updates.list.map(item => item.id), [5, 4, 99]);
assert.equal(updates.list[0].status, 'completed');
assert.deepEqual(updates.missing, []);
assert.deepEqual(updateLoadedCloud([batch(5), batch(99)], [], [5]).missing, [5]);
await assert.rejects(() => fetchCloudPages([1, 2], 2, async number => number === 1 ? first : Promise.reject(new Error('offline'))), /offline/);
await assert.rejects(() => fetchCloudPages([1], 2, async () => page([batch(1, { space_id: 3 })])), /当前空间/);
await assert.rejects(() => fetchCloudPages([1, 2, 3, 4], 2, async number => page([], number)), /分页范围无效/);
// Mock a real offset-paginated service, including mutation between calls.
const makeServer = initial => {
  let rows = initial;
  let offline = false;
  const calls = [];
  return {
    get rows() { return rows; }, set rows(value) { rows = value; },
    set offline(value) { offline = value; }, calls,
    async fetch(number) {
      calls.push(number);
      if (offline) throw new Error('offline');
      const total = rows.length;
      if (number > Math.max(1, Math.ceil(total / 20))) throw new Error('invalid page');
      return page(rows.slice((number - 1) * 20, number * 20), number, total,
        number * 20 < total ? '?next' : null);
    },
  };
};
const runRebuild = async (server, wanted) => {
  let state = startCloudRebuild(wanted);
  let ticks = 0;
  while (!rebuildComplete(state)) {
    assert.ok(++ticks < 100, 'rebuild must converge with a bounded number of batches');
    const numbers = rebuildNextPages(state);
    assert.ok(numbers.length >= 1 && numbers.length <= 3);
    state = advanceCloudRebuild(state, await fetchCloudPages(numbers, 2, n => server.fetch(n)), 2);
  }
  return state.verified;
};
const ids = pages => pages.flatMap(item => item.list.map(row => row.id));
const server = makeServer(Array.from({ length: 45 }, (_, i) => batch(100 - i)));
let loaded = await runRebuild(server, 2);
assert.equal(ids(loaded).length, 40);
assert.equal(loaded.at(-1).pagination.next, '?next');
const oldIds = ids(loaded);
server.rows = [batch(101), ...server.rows];
assert.equal(sameCloudLayout(loaded[0], await server.fetch(1)), false);
loaded = await runRebuild(server, 2);
assert.deepEqual(ids(loaded), server.rows.slice(0, 40).map(row => row.id));
assert.equal(ids(loaded).includes(oldIds.at(-1)), false, 'records shifted outside prefix must not be retained');
assert.equal(ids(loaded)[0], 101);
// Stable load-more is constant-cost: two boundary pages plus the next page.
const many = makeServer(Array.from({ length: 540 }, (_, i) => batch(1000 - i)));
let prefix = [await many.fetch(1)];
for (let wanted = 2; wanted <= 26; wanted++) {
  const plan = [...new Set([1, prefix.length, prefix.length + 1])];
  assert.ok(plan.length <= 3);
  const next = appendCloudPages(prefix, await fetchCloudPages(plan, 2, n => many.fetch(n)), 2);
  assert.ok(next); prefix = next;
}
assert.equal(prefix.length, 26);
assert.equal(ids(prefix).length, 520);
assert.ok(many.calls.length <= 76, 'normal load-more should not rescan the entire prefix');
const beforeDrift = prefix;
many.rows = [batch(1001), ...many.rows];
const planAfterInsert = [...new Set([1, prefix.length, prefix.length + 1])];
assert.equal(appendCloudPages(prefix, await fetchCloudPages(planAfterInsert, 2, n => many.fetch(n)), 2), null);
assert.equal(prefix, beforeDrift, 'drift must preserve the displayed prefix until reconstruction');
const repaired = await runRebuild(many, 27);
assert.equal(repaired.length, 27);
assert.equal(ids(repaired)[0], 1001);
// Duplicating an ID already present on a different page must also fall back.
const dupServer = makeServer(Array.from({ length: 60 }, (_, i) => batch(500 - i)));
const prefix2 = [await dupServer.fetch(1), await dupServer.fetch(2)];
dupServer.rows[40] = dupServer.rows[0];
assert.equal(appendCloudPages(prefix2, await fetchCloudPages([1, 2, 3], 2, n => dupServer.fetch(n)), 2), null);
// Deletion shrinks three pages to one: never request an invalid former page.
server.rows = server.rows.slice(0, 17);
server.calls.length = 0;
loaded = await runRebuild(server, 3);
assert.deepEqual(ids(loaded), server.rows.map(row => row.id));
assert.equal(loaded.length, 1);
assert.ok(server.calls.every(n => n === 1));
assert.equal(loaded[0].pagination.next, null);
// Membership stability permits status refresh without spurious reconstruction.
const stable = await server.fetch(1);
server.rows = server.rows.map((row, index) => index ? row : { ...row, status: 'processing' });
const statusUpdate = await server.fetch(1);
assert.equal(sameCloudLayout(stable, statusUpdate), true);
assert.equal(statusUpdate.list[0].status, 'processing');
assert.equal(refreshInterval(false, false), 30000); // idle pages still poll
assert.equal(refreshInterval(true, false), 10000);
assert.equal(refreshInterval(false, true), 2000);
assert.ok(pollPagePlan(26, 23, [], 15).includes(1));
// Offline does not publish a partial snapshot.
const committed = loaded;
server.offline = true;
await assert.rejects(runRebuild(server, 2), /offline/);
assert.equal(loaded, committed);
server.offline = false;
assert.deepEqual(ids(await runRebuild(server, 2)), ids(committed));
// A changed total between requests invalidates the whole attempted prefix.
server.rows = Array.from({ length: 42 }, (_, i) => batch(200 - i));
let rebuilding = startCloudRebuild(3);
rebuilding = advanceCloudRebuild(rebuilding, [await server.fetch(1)], 2);
server.rows.unshift(batch(201));
await assert.rejects(async () => advanceCloudRebuild(rebuilding, await fetchCloudPages(rebuildNextPages(rebuilding), 2, n => server.fetch(n)), 2), /变化/);
// Equal totals are insufficient: a changed membership during verification fails.
rebuilding = startCloudRebuild(2);
rebuilding = advanceCloudRebuild(rebuilding, [await server.fetch(1)], 2);
rebuilding = advanceCloudRebuild(rebuilding, [await server.fetch(2)], 2);
assert.equal(rebuilding.phase, 'verify');
server.rows = [batch(202), ...server.rows.slice(0, -1)];
await assert.rejects(async () => advanceCloudRebuild(rebuilding, await fetchCloudPages(rebuildNextPages(rebuilding), 2, n => server.fetch(n)), 2), /变化/);
// Duplicate IDs within or across pages must never be committed.
const duplicateRows = Array.from({ length: 40 }, (_, i) => batch(300 - i));
duplicateRows[1] = duplicateRows[0];
server.rows = duplicateRows;
await assert.rejects(runRebuild(server, 2), /重复/);
duplicateRows[1] = batch(299); duplicateRows[20] = duplicateRows[0];
await assert.rejects(runRebuild(server, 2), /重复/);
assert.throws(() => validateCloudPrefixPage(page([batch(1), batch(1)], 1, 2), 1, 2), /重复/);
// Scope, identity, request generation and retry/load-more revisions guard results.
const origin = settingsScope(settings);
assert.equal(sameCloudContext(origin, 42, origin, 42), true);
assert.equal(sameCloudContext(origin, 42, origin, 43), false);
assert.equal(sameCloudContext(origin, 42, origin, null), false);
assert.equal(sameCloudContext(origin, null, origin, null), false);
assert.equal(sameCloudContext(origin, 42, settingsScope({ ...settings, devify: { ...settings.devify, active_space_id: 3 } }), 42), false);
assert.equal(sameCloudContext(origin, 42, settingsScope({ ...settings, devify: { ...settings.devify, base_url: 'https://other.test' } }), 42), false);
assert.equal(sameCloudContext(origin, 42, settingsScope({ ...settings, active_account_id: 'other' }), 42), false);
assert.equal(requestIsCurrent(1, 1, 1, 1, false), true);
assert.equal(requestIsCurrent(1, 2, 1, 1, false), false);
assert.equal(requestIsCurrent(1, 1, 1, 2, false), false);
assert.equal(requestIsCurrent(1, 1, 1, 1, true), false);
// Failure waits for other in-flight requests before releasing the batch lock.
let release;
const slow = new Promise(resolve => { release = resolve; });
let finished = false;
const failedFetchBatch = fetchCloudPages([1, 2], 2, n => n === 1 ? Promise.reject(new Error('offline')) : slow)
  .catch(() => { finished = true; });
await Promise.resolve(); await Promise.resolve();
assert.equal(finished, false);
release(page([], 2, 0));
await failedFetchBatch;
assert.equal(finished, true);
console.log('task cloud checks passed');
