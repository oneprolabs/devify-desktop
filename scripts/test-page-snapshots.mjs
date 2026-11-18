import assert from 'node:assert/strict';
import { clearPageSnapshots, pageSnapshotGeneration, readPageSnapshot, savePageSnapshot } from '../src/pageSnapshots.ts';

const previousLogin = pageSnapshotGeneration();
savePageSnapshot('user-a', ['old'], previousLogin);
assert.deepEqual(readPageSnapshot('user-a'), ['old']);
clearPageSnapshots();
savePageSnapshot('user-a', ['late response'], previousLogin);
assert.equal(readPageSnapshot('user-a'), undefined);
savePageSnapshot('user-b', ['current'], pageSnapshotGeneration());
assert.deepEqual(readPageSnapshot('user-b'), ['current']);
console.log('page snapshot invalidation passed');
