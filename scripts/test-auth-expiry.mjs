import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

const source = fs.readFileSync(new URL('../src/authEvents.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ES2020 } }).outputText;
const { shouldApplyAuthExpired } = await import(`data:text/javascript,${encodeURIComponent(js)}`);

assert.equal(await shouldApplyAuthExpired(async () => ({ authenticated: false }), () => true), true);
assert.equal(await shouldApplyAuthExpired(async () => ({ authenticated: true }), () => true), false);
assert.equal(await shouldApplyAuthExpired(async () => ({ authenticated: false, unavailable: true }), () => true), false);
assert.equal(await shouldApplyAuthExpired(async () => { throw new Error('unknown'); }, () => true), false);
let epoch = 1;
let resolveStatus;
const pending = shouldApplyAuthExpired(() => new Promise(resolve => { resolveStatus = resolve; }), () => epoch === 1);
epoch = 2; // Another login completed before the old status response arrived.
resolveStatus({ authenticated: false });
assert.equal(await pending, false);
console.log('auth expiry checks passed');
