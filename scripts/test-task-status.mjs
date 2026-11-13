import assert from 'node:assert/strict';
import { localTaskStatus } from '../src/taskStatus.ts';
assert.equal(localTaskStatus({ status: 'completed' }, []), 'completed');
assert.equal(localTaskStatus({ status: 'processing' }, []), 'processing');
assert.equal(localTaskStatus({ status: 'failed' }, []), 'failed');
assert.equal(localTaskStatus({ status: 'queued', manual_group: 'g', manual_total: 2 }, [{ status: 'completed' }, { status: 'completed' }]), 'completed');
assert.equal(localTaskStatus({ status: 'queued', manual_group: 'g', manual_total: 2 }, [{ status: 'completed' }]), 'processing');
assert.equal(localTaskStatus({ status: 'queued', manual_group: 'g', manual_total: 2 }, [{ status: 'completed' }, { status: 'failed' }]), 'failed');
console.log('task status checks passed');
