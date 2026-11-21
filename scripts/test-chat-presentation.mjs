import assert from 'node:assert/strict';
import { currentSessionSummaries, eventViewState, syncErrorMessage } from '../src/features/chat/chatPresentation.ts';

const rows = [{ account_id: 'x', sessions: [{ username: 'a' }] }, { account_id: 'x', sessions: [{ username: 'a' }, { username: 'b' }] }, { account_id: 'y', sessions: [{ username: 'a' }] }];
assert.deepEqual(currentSessionSummaries(rows, 'x', 'a'), [rows[0], rows[1]]);
assert.deepEqual(currentSessionSummaries(rows, 'x', null), []);
assert.deepEqual(currentSessionSummaries(rows, null, 'a'), []);
assert.equal(eventViewState(true, '', 0), 'loading');
assert.equal(eventViewState(false, 'offline', 0), 'error');
assert.equal(eventViewState(false, '', 0), 'empty');
assert.equal(eventViewState(false, 'offline', 1), 'results');
assert.match(syncErrorMessage('该账号正在上传，请稍后重试。'), /后台上传/);
assert.match(syncErrorMessage('network'), /network/);
console.log('chat presentation tests passed');
