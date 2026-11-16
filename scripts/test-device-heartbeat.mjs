import assert from 'node:assert/strict'
import fs from 'node:fs'
import ts from 'typescript'

const source = fs.readFileSync(new URL('../src/devifyDevice.ts', import.meta.url), 'utf8')
  .replace("import { invoke } from '@tauri-apps/api/core'", 'const invoke = globalThis.__testDeviceInvoke')
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ES2022, target: ts.ScriptTarget.ES2022 } }).outputText
let calls = 0
let finish
let fail = false
globalThis.__testDeviceInvoke = () => { calls++; return fail ? Promise.reject(new Error("offline")) : new Promise(resolve => { finish = resolve }) }
const { registerDevifyDevice } = await import(`data:text/javascript;base64,${Buffer.from(js).toString('base64')}`)
const first = registerDevifyDevice()
assert.equal(registerDevifyDevice(), first)
assert.equal(calls, 1)
finish({ device_id: 'test' })
await first
const second = registerDevifyDevice()
assert.equal(calls, 2)
finish({ device_id: 'test' })
await second
fail = true
assert.equal(await registerDevifyDevice(), null)
fail = false
const retry = registerDevifyDevice()
assert.equal(calls, 4)
finish({ device_id: "test" })
await retry
globalThis.__testDeviceInvoke = undefined
console.log('device heartbeat deduplication and renewal passed')
